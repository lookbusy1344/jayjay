import JayJayCore
import JayJayDiffUI
import SwiftUI

extension DiffSection: DiffGutterEditActions {
    var currentSelectedLineRange: ClosedRange<Int>? {
        selectedLineRange
    }

    var canOpenDiffEdit: Bool {
        onOpenDiffEdit != nil && hunk.projection == nil
    }

    var canAbandonSelectedLines: Bool {
        guard isWorkingCopy, hunk.projection == nil, !hunk.hunkType.hasSource, let renderedDiff else { return false }
        // Conflicts materialize as marker text the core partitioner would treat as literal content; the core guard rejects them, so hide the action like the GPUI shell instead of offering an error.
        return !renderedDiff.flags.hasConflicts
    }

    var canEditFile: Bool {
        onEditFile != nil && hasCurrentRenderableDiff && loadedDiff?.content.supportsFileEditor == true
    }

    var externalEditorName: String? {
        onOpenInExternalEditor != nil && settings.externalEditorOpensAtLine ? settings.externalEditor.title : nil
    }

    func editFile(atLine line: UInt32) {
        onEditFile?(line)
    }

    func openInExternalEditor(atLine line: UInt32) {
        onOpenInExternalEditor?(line)
    }

    func didSelectLines(_ lineRange: ClosedRange<Int>) {
        selectedLineRange = lineRange
    }

    func openDiffEdit() {
        onOpenDiffEdit?()
    }

    func abandonSelectedLines(in lineRange: ClosedRange<Int>) {
        abandonSelectedLines(lineRange: lineRange)
    }

    private func abandonSelectedLines(lineRange: ClosedRange<Int>) {
        guard let actions,
              let repo,
              let rev,
              let loadedDiff
        else { return }

        let oldContent = loadedOldContent ?? hunk.oldContent
        let newContent = loadedNewContent ?? hunk.newContent
        let selectedKeys = loadedDiff.changedLineKeys(selecting: lineRange)
        guard !selectedKeys.isEmpty else { return }

        let path = hunk.path
        let oldPath = hunk.oldPath
        let hunkType = hunk.hunkType
        let ignoreWhitespace = settings.ignoreWhitespace
        Task.detached { [repo] in
            let fullDiff = repo.computeNativeDiffFull(
                path: path,
                oldContent: oldContent ?? "",
                newContent: newContent ?? "",
                ignoreWhitespace: ignoreWhitespace
            )
            let ranges = DiffEditRange.ranges(holding: selectedKeys, in: fullDiff.lines)
            guard !ranges.isEmpty else { return }

            await MainActor.run {
                actions.applyDiffSelection(
                    rev: rev,
                    destination: .removeFromSource,
                    selections: [
                        DiffEditFileSelection(
                            path: path,
                            oldPath: oldPath,
                            oldContent: oldContent,
                            newContent: newContent,
                            hunkType: hunkType,
                            lineRanges: ranges
                        )
                    ],
                    message: "",
                    ignoreWhitespace: ignoreWhitespace
                )
            }
        }
    }
}

extension DiffSectionLoadedDiff {
    /// Keys of the changed rows at rendered display-line numbers `lineRange`.
    func changedLineKeys(selecting lineRange: ClosedRange<Int>) -> Set<String> {
        Set((rows?.lines ?? []).enumerated().compactMap { index, line in
            guard lineRange.contains(index + 1), line.style == .added || line.style == .removed else { return nil }
            return line.diffEditKey
        })
    }
}

extension DiffEditRange {
    /// 1-based ranges of `lines` holding the rows with these keys. Rendered rows list each changed run removed-before-added while a raw diff interleaves word pairs, so rows map by key, not by index.
    static func ranges(holding keys: Set<String>, in lines: [DiffLine]) -> [DiffEditRange] {
        let indices = lines.enumerated().compactMap { index, line in
            keys.contains(line.diffEditKey) ? index + 1 : nil
        }
        guard let first = indices.first else { return [] }

        var ranges: [DiffEditRange] = []
        var start = first
        var previous = first

        for index in indices.dropFirst() {
            if index == previous + 1 {
                previous = index
                continue
            }
            ranges.append(DiffEditRange(startLine: UInt32(start), endLine: UInt32(previous)))
            start = index
            previous = index
        }

        ranges.append(DiffEditRange(startLine: UInt32(start), endLine: UInt32(previous)))
        return ranges
    }
}

private extension DiffLine {
    var diffEditKey: String {
        let style = switch style {
            case .added: "added"
            case .removed: "removed"
            case .context: "context"
            case .separator: "separator"
            case .unchanged: "unchanged"
        }
        return "\(style)|\(oldLineNo.map(String.init) ?? "-")|\(newLineNo.map(String.init) ?? "-")"
    }
}
