//! Scratch buffer for transient message serialization.
//!
//! Shared buffer for replay, gap fills, and sends with history disabled.
//! Pending bytes must be consumed before the next write. Debug builds detect
//! writes while the buffer is marked dirty; release builds do not check this.

// take_pending clears the dirty flag before the transport write. The IO loop
// must complete that write before permitting another serialization.

pub(super) struct ScratchBuffer {
    buf: Box<[u8]>,
    /// Tracks whether `buf` holds bytes still pending a TCP write.
    /// Set by [`Self::write`] on successful serialization, cleared by
    /// [`Self::mark_clean`] once the bytes are written.
    #[cfg(debug_assertions)]
    dirty: bool,
}

impl ScratchBuffer {
    pub(super) fn new(size: usize) -> Self {
        Self {
            buf: vec![0u8; size].into_boxed_slice(),
            #[cfg(debug_assertions)]
            dirty: false,
        }
    }

    /// Read-only view of the buffer; the dirty flag is not affected.
    pub(super) fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    /// Mark the buffer free for the next writer. Called after the
    /// bytes previously written via [`Self::write`] have been
    /// consumed (typically by the IO loop's `write_all` to TCP).
    /// No-op in release builds.
    pub(super) fn mark_clean(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.dirty = false;
        }
    }

    /// Serialize into the buffer via `f`.
    ///
    /// Panics under `#[cfg(debug_assertions)]` if the buffer still
    /// holds bytes from a previous write that have not been consumed
    /// via [`Self::mark_clean`]. On success, marks the buffer dirty.
    /// On `Err` the buffer may have been partially written, but no
    /// caller-visible reference points at it, so the dirty flag stays
    /// clear and the next write is free to reuse the buffer.
    // `allow`, not `expect`: the assertion is compiled out in release, so
    // the lint has nothing to fire on there and an expectation would go
    // unfulfilled. `debug_assert!` is not an option either - it type-checks
    // its expression in release, where `dirty` does not exist.
    #[allow(
        clippy::panic_in_result_fn,
        reason = "debug-only check of an internal aliasing invariant, documented above"
    )]
    pub(super) fn write<F, E>(&mut self, f: F) -> Result<usize, E>
    where
        F: FnOnce(&mut [u8]) -> Result<usize, E>,
    {
        #[cfg(debug_assertions)]
        assert!(
            !self.dirty,
            "scratch buffer aliasing: previous serialize is still queued for write"
        );
        let result = f(&mut self.buf);
        #[cfg(debug_assertions)]
        if result.is_ok() {
            self.dirty = true;
        }
        result
    }

    /// Mutable access to the underlying buffer. Used by tests to plant
    /// transient bytes for `flush_output` testing. The dirty flag is
    /// not changed - tests are responsible for calling
    /// [`Self::mark_clean`] if they want to follow the production
    /// discipline.
    #[cfg(test)]
    pub(super) fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buf
    }
}
