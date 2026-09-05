import JayJayCore

final class MainActorLogGraphObserver: LogGraphObserver, @unchecked Sendable {
    private let continuation: AsyncStream<LogGraphEvent>.Continuation
    private let consumer: Task<Void, Never>

    init(handler: @escaping @MainActor @Sendable (LogGraphEvent) -> Void) {
        let (stream, continuation) = AsyncStream<LogGraphEvent>.makeStream()
        self.continuation = continuation
        consumer = Task { @MainActor in
            for await event in stream {
                handler(event)
            }
        }
    }

    deinit {
        continuation.finish()
    }

    nonisolated func onEvent(event: LogGraphEvent) {
        continuation.yield(event)
    }
}
