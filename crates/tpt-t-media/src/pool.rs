//! Custom slab allocator / memory pool for video frames and sensor packets
//! (spec §5.3 "Media & Telemetry").
//!
//! A [`FramePool`] owns one contiguous allocation carved into fixed-size
//! blocks. Allocation and release are O(1) over a free-index stack and never
//! touch the heap after construction — the same "pre-allocate, then run
//! lock/alloc-free" discipline the rest of the hot path follows. Each live
//! block is a [`Block`] guard that returns its storage to the pool on drop,
//! so a frame's backing bytes are recycled deterministically.
//!
//! The media pipeline runs on one pinned core (spec §3.1), so at most one
//! block is mutably borrowed at a time; the borrow checker enforces that the
//! pool cannot be re-entered while a frame is outstanding.

/// Pixel format of a frame in the pool / a captured buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixFmt {
    /// 8-bit grayscale.
    GrayY8 = 0,
    /// Packed 24-bit RGB.
    Rgb888 = 1,
    /// Semi-planar NV12 (ltrate Y plane + interleaved UV).
    Nv12 = 2,
}

impl PixFmt {
    /// Bytes per pixel for planar/packed formats (`Nv12` uses
    /// [`min_buffer_len`](Self::min_buffer_len) instead).
    #[inline]
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixFmt::GrayY8 => 1,
            PixFmt::Rgb888 => 3,
            PixFmt::Nv12 => 2,
        }
    }

    /// Minimum buffer bytes for a `w×h` frame in this format.
    #[inline]
    pub fn min_buffer_len(self, w: u32, h: u32) -> usize {
        match self {
            // NV12: w*h luma + (w/2)*(h/2)*2 interleaved chroma = w*h*3/2.
            PixFmt::Nv12 => (w as usize) * (h as usize) * 3 / 2,
            other => other.bytes_per_pixel() * w as usize * h as usize,
        }
    }
}

/// Metadata describing a frame's geometry and timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct FrameMeta {
    /// Monotonic frame sequence.
    pub seq: u64,
    /// Capture timestamp (UNIX ns).
    pub timestamp_ns: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row stride in bytes.
    pub stride: u32,
    /// [`PixFmt`] discriminant.
    pub format: u32,
}

impl FrameMeta {
    /// Constructs metadata, inferring stride from `format`/`width`.
    pub fn new(seq: u64, timestamp_ns: u64, width: u32, height: u32, format: PixFmt) -> Self {
        let stride = match format {
            PixFmt::Nv12 => width, // Y plane stride; chroma follows contiguously
            other => other.bytes_per_pixel() as u32 * width,
        };
        Self {
            seq,
            timestamp_ns,
            width,
            height,
            stride,
            format: format as u32,
        }
    }

    /// The frame's pixel format.
    #[inline]
    pub fn pixfmt(self) -> PixFmt {
        match self.format {
            0 => PixFmt::GrayY8,
            1 => PixFmt::Rgb888,
            _ => PixFmt::Nv12,
        }
    }
}

/// A slab of fixed-size blocks. Build once, then [`alloc`](FramePool::alloc)
/// and drop blocks for the rest of the program's life.
pub struct FramePool {
    slab: Vec<u8>,
    block_len: usize,
    free: Vec<u32>,
    in_use: usize,
}

impl FramePool {
    /// Creates a pool of `blocks` blocks, each `block_len` bytes. The backing
    /// slab is allocated once here and never reallocated.
    pub fn new(block_len: usize, blocks: usize) -> Self {
        assert!(block_len > 0 && blocks > 0, "pool needs at least one block");
        let mut free = Vec::with_capacity(blocks);
        // Push high→low so allocation hands out low indices first (cache-hot).
        for i in (0..blocks as u32).rev() {
            free.push(i);
        }
        Self {
            slab: vec![0u8; block_len * blocks],
            block_len,
            free,
            in_use: 0,
        }
    }

    /// Size in bytes of every block.
    #[inline]
    pub fn block_len(&self) -> usize {
        self.block_len
    }

    /// Total blocks.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.free.len() + self.in_use
    }

    /// Currently free blocks.
    #[inline]
    pub fn available(&self) -> usize {
        self.free.len()
    }

    /// Currently allocated blocks.
    #[inline]
    pub fn in_use(&self) -> usize {
        self.in_use
    }

    /// Pops a free block, returning a guard that borrows the pool and reclaims
    /// the storage on drop. Returns `None` when exhausted — callers shed
    /// frames rather than allocate.
    pub fn alloc(&mut self) -> Option<Block<'_>> {
        let idx = self.free.pop()?;
        self.in_use += 1;
        Some(Block { pool: self, idx })
    }
}

/// A borrowed block of the pool. Dereferences to its mutable byte range;
/// dropping it returns the block to the free list.
pub struct Block<'p> {
    pool: &'p mut FramePool,
    idx: u32,
}

impl core::ops::Deref for Block<'_> {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        let s = self.idx as usize * self.pool.block_len;
        &self.pool.slab[s..s + self.pool.block_len]
    }
}

impl core::ops::DerefMut for Block<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        let s = self.idx as usize * self.pool.block_len;
        &mut self.pool.slab[s..s + self.pool.block_len]
    }
}

impl Drop for Block<'_> {
    fn drop(&mut self) {
        self.pool.in_use -= 1;
        self.pool.free.push(self.idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_len_is_honored_and_reused() {
        let mut pool = FramePool::new(64, 4);
        assert_eq!(pool.block_len(), 64);
        assert_eq!(pool.capacity(), 4);
        assert_eq!(pool.available(), 4);

        // The pool is single-borrow: one block at a time, reclaimed on drop.
        // (The per-core media pipeline processes one frame before the next,
        // so a block is never needed while another is outstanding.)
        let len = {
            let mut a = pool.alloc().unwrap();
            a[0] = 1;
            a[63] = 2;
            assert_eq!(a[0], 1);
            a.len()
        };
        assert_eq!(len, 64);
        assert_eq!(pool.available(), 4, "block reclaimed on drop");
    }

    #[test]
    fn blocks_are_reusable_after_drop() {
        let mut pool = FramePool::new(32, 2);
        assert_eq!(pool.available(), 2);
        {
            let _b = pool.alloc().unwrap();
            assert_eq!(_b.len(), 32);
        }
        // Reuse after reclaim still hands out a full block.
        {
            let c = pool.alloc().unwrap();
            assert_eq!(c.len(), 32);
        }
        {
            let d = pool.alloc().unwrap();
            assert_eq!(d.len(), 32);
        }
        assert_eq!(pool.available(), 2);
    }

    #[test]
    fn single_block_pool_round_trips() {
        let mut pool = FramePool::new(16, 1);
        {
            let _a = pool.alloc().unwrap();
            assert_eq!(_a.len(), 16);
        }
        // After release, the single block is available again.
        assert!(pool.alloc().is_some());
    }

    #[test]
    fn format_buffer_sizing_matches_expectation() {
        assert_eq!(PixFmt::Rgb888.min_buffer_len(100, 50), 100 * 50 * 3);
        assert_eq!(PixFmt::GrayY8.min_buffer_len(100, 50), 100 * 50);
        // NV12 100x50 → 100*50 + 50*25*2 = 7500.
        assert_eq!(PixFmt::Nv12.min_buffer_len(100, 50), 7500);
        let meta = FrameMeta::new(1, 2, 100, 50, PixFmt::Rgb888);
        assert_eq!(meta.stride, 300);
        assert_eq!(meta.pixfmt(), PixFmt::Rgb888);
    }
}
