import Darwin
@testable import JayJay
import JayJayCore
import JayJayDiffUI
import XCTest

/// End-to-end timings and retained memory for the diff paths in `plans/ffi-diff-transport.md`. Run through `just shell::bench-ffi`, which builds the fixture repo first.
final class DiffFFIBenchmarks: XCTestCase {
    private static let fixturePath = "/tmp/jayjay-ffi-bench"
    private static let resultsPath = "/tmp/jayjay-ffi-bench-results.tsv"
    private static let timeBudget = Duration.seconds(1)
    private static let minRuns = 2
    private static let maxRuns = 10
    /// Four widths each way keep the dense sweep near a second; results are per width change.
    private static let sbsWidths: [UInt32] = Array(stride(from: 40, through: 85, by: 15))
    private static let expansionBasis = "bench"

    private static let repo: JayJayRepo = {
        guard let repo = try? JayJayRepo.open(path: fixturePath) else {
            fatalError("missing fixture at \(fixturePath); run just shell::bench-ffi")
        }
        return repo
    }()

    /// The runner relaunches the host after a crash and calls this again, so only a missing file gets the header.
    override static func setUp() {
        super.setUp()
        guard !FileManager.default.fileExists(atPath: resultsPath) else { return }
        FileManager.default.createFile(
            atPath: resultsPath,
            contents: Data("benchmark\tscenario\tmedian ms\truns\tretained MiB\tretained blocks\tfootprint MiB\n".utf8)
        )
    }

    func testOpenUnifiedSparse() async throws {
        try await openUnified("sparse")
    }

    func testOpenUnifiedDense() async throws {
        try await openUnified("dense")
    }

    func testResizeSideBySideDense() async throws {
        try await resizeSideBySide("dense")
    }

    func testResizeSideBySideWide() async throws {
        try await resizeSideBySide("wide")
    }

    private func openUnified(_ scenario: String) async throws {
        let hunks = try hunks(scenario)
        XCTAssertEqual(hunks.count, 1)
        try await bench("open_unified", scenario) {
            let store = DiffStore()
            var prepared: [DiffSectionLoadedDiff] = []
            for hunk in hunks {
                let loaded = await store.loadDiff(hunk: hunk, rev: Self.rev(scenario), repo: Self.repo)
                let cached = try XCTUnwrap(loaded)
                prepared.append(DiffSectionLoadedDiff(cached, path: hunk.path, basis: scenario, hunk: hunk))
            }
            return (store, prepared)
        }
    }

    private func resizeSideBySide(_ scenario: String) async throws {
        let hunk = try XCTUnwrap(hunks(scenario).first)
        let cached = try await Self.load(hunk, scenario)
        let rows = cached.loaded.sideBySideRows()
        try await bench("resize_sbs (per width)", scenario, iterations: Self.sbsWidths.count * 2) {
            var rowCount = 0
            for cols in Self.sbsWidths + Self.sbsWidths.reversed() {
                rowCount += SideBySideRowSource.visualRows(
                    rows: rows, layout: cached.loaded.sideBySideLayout(oldCols: cols, newCols: cols)
                ).count
            }
            return rowCount
        }
    }

    func testExpandContext() async throws {
        let scenario = "sparse"
        let hunk = try XCTUnwrap(hunks(scenario).first)
        let cached = try await Self.load(hunk, scenario)
        try await bench("expand_context", scenario) {
            let session = ContextExpansionSession()
            let attempt = try XCTUnwrap(session.begin(basis: Self.expansionBasis, request: .allRegions))
            let outcome = try XCTUnwrap(attempt.run(base: cached.loaded))
            guard case let .applied(diff, _, _, _) = session.finish(basis: Self.expansionBasis, outcome: outcome) else {
                throw XCTSkip("expansion was not applied")
            }
            return (DiffStore.CachedDiff(loaded: diff, content: cached.content), diff.displayRows())
        }
    }

    func testPreload() async throws {
        let scenario = "many"
        let hunks = try hunks(scenario)
        XCTAssertEqual(hunks.count, 40)
        try await bench("preload", scenario) {
            let store = DiffStore()
            for hunk in hunks {
                let loaded = await store.loadDiff(hunk: hunk, rev: Self.rev(scenario), repo: Self.repo)
                XCTAssertNotNil(loaded)
            }
            return store
        }
    }

    // MARK: - Harness

    private static func rev(_ scenario: String) -> String {
        "bench-\(scenario)"
    }

    private static func load(_ hunk: DiffHunk, _ scenario: String) async throws -> DiffStore.CachedDiff {
        let loaded = await DiffStore().loadDiff(hunk: hunk, rev: rev(scenario), repo: repo)
        return try XCTUnwrap(loaded)
    }

    private func hunks(_ scenario: String) throws -> [DiffHunk] {
        try Self.repo.showSummary(rev: Self.rev(scenario)).diff
    }

    /// One warm-up, then timed runs until the budget is spent; retained memory is read on the last run while its result is still alive.
    /// `iterations` divides the reported time when one run repeats an operation, so the result is per operation.
    private func bench(
        _ name: String,
        _ scenario: String,
        iterations: Int = 1,
        _ work: () async throws -> some Any
    ) async throws {
        _ = try await work()
        var times: [Duration] = []
        let started = ContinuousClock.now
        var retained = MemoryDelta()
        while times.count < Self.minRuns || (times.count < Self.maxRuns && started.duration(to: .now) < Self.timeBudget) {
            let before = MemorySample()
            let runStart = ContinuousClock.now
            let result = try await work()
            times.append(runStart.duration(to: .now))
            retained = MemorySample().delta(from: before)
            withExtendedLifetime(result) {}
        }
        let median = times.sorted()[times.count / 2] / iterations
        let line = [
            name, scenario,
            String(format: "%.1f", median.milliseconds),
            String(times.count),
            String(format: "%.2f", retained.mebibytes),
            String(retained.blocks),
            String(format: "%.2f", retained.footprintMebibytes)
        ].joined(separator: "\t")
        print("FFIBENCH\t\(line)")
        if let handle = FileHandle(forWritingAtPath: Self.resultsPath) {
            handle.seekToEndOfFile()
            handle.write(Data((line + "\n").utf8))
            try handle.close()
        }
    }
}

private struct MemorySample {
    let bytesInUse: Int
    let blocksInUse: Int
    let footprint: UInt64

    init() {
        var stats = malloc_statistics_t()
        malloc_zone_statistics(nil, &stats)
        bytesInUse = Int(stats.size_in_use)
        blocksInUse = Int(stats.blocks_in_use)
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let status = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        footprint = status == KERN_SUCCESS ? info.phys_footprint : 0
    }

    func delta(from before: MemorySample) -> MemoryDelta {
        MemoryDelta(
            bytes: bytesInUse - before.bytesInUse,
            blocks: blocksInUse - before.blocksInUse,
            footprint: Int64(footprint) - Int64(before.footprint)
        )
    }
}

private struct MemoryDelta {
    var bytes = 0
    var blocks = 0
    var footprint: Int64 = 0

    private static let mebibyte = 1024.0 * 1024.0

    var mebibytes: Double {
        Double(bytes) / Self.mebibyte
    }

    var footprintMebibytes: Double {
        Double(footprint) / Self.mebibyte
    }
}

private extension Duration {
    var milliseconds: Double {
        let (seconds, attoseconds) = components
        return Double(seconds) * 1000 + Double(attoseconds) / 1e15
    }
}
