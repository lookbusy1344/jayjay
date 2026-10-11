/// Stack reserved per recursion level, with headroom for unoptimized builds; one level takes about 720 bytes in the optimized macOS build.
const STACK_PER_LEVEL: usize = 4 << 10;
/// Stack for everything outside the recursion: anchor search, the Myers fallback and the caller's frames.
const STACK_BASE: usize = 256 << 10;
/// The ceiling `jayjay-core`'s `on_worker_stack` uses: about 45,000 levels at the optimized frame size, and small enough for Windows, which commits a fiber stack up front.
const MAX_STACK: usize = 32 << 20;

/// `similar`'s histogram diff recurses once per matched block, so its depth is bounded by the shorter side's token count. Shell worker threads (Swift's cooperative pool) have 512 KiB of stack, which a few thousand scattered changes overflow, so a caller with less than that bound left continues on a grown stack.
pub(crate) fn with_histogram_stack<R>(max_depth: usize, diff: impl FnOnce() -> R) -> R {
    #[cfg(not(target_family = "wasm"))]
    {
        let needed = STACK_BASE
            .saturating_add(max_depth.saturating_mul(STACK_PER_LEVEL))
            .min(MAX_STACK);
        stacker::maybe_grow(needed, needed, diff)
    }
    #[cfg(target_family = "wasm")]
    diff()
}
