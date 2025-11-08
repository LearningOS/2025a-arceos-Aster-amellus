#![no_std]

use core::{mem::MaybeUninit};

use allocator::{BaseAllocator, ByteAllocator, PageAllocator};

#[inline]
pub const fn align_down(pos: usize, align: usize) -> usize {
    pos & !(align - 1)
}

#[inline]
pub const fn align_up(pos: usize, align: usize) -> usize {
    (pos + align - 1) & !(align - 1)
}

/// Early memory allocator
/// Use it before formal bytes-allocator and pages-allocator can work!
/// This is a double-end memory range:
/// - Alloc bytes forward
/// - Alloc pages backward
///
/// [ bytes-used | avail-area | pages-used ]
/// |            | -->    <-- |            |
/// start       b_pos        p_pos       end
///
/// For bytes area, 'count' records number of allocations.
/// When it goes down to ZERO, free bytes-used area.
/// For pages area, it will never be freed!
///
pub struct EarlyAllocator<const SIZE: usize> {
    arena: [MaybeUninit<u8>; SIZE],
    b_start: usize,
    b_pos: usize,
    p_pos: usize,
    p_end: usize,
    b_count: usize,
}

impl<const SIZE: usize> EarlyAllocator<SIZE> {
    // Define Page Size
    pub const PAGE_SIZE: usize = 4096;

    // Create a new, empty EarlyAllocator
    pub const fn new() -> Self {
        const UNINIT: MaybeUninit<u8> = MaybeUninit::uninit();
        Self {
            arena: [UNINIT; SIZE],
            b_start: 0,
            b_pos: 0,
            p_pos: SIZE,
            p_end: 0,
            b_count: 0,
        }
    }
}

impl<const SIZE: usize> BaseAllocator for EarlyAllocator<SIZE> {
    fn init(&mut self, _start: usize, _size: usize) {
        let start = 0;
        let end = SIZE;

        assert!(end <= SIZE, "EarlyAllocator init range exceeds arena size");

        self.b_start = start;
        self.b_pos = start;
        self.p_pos = end;
        self.p_end = end;
        self.b_count = 0;
    }

    fn add_memory(&mut self, start: usize, size: usize) -> allocator::AllocResult {
        Err(allocator::AllocError::InvalidParam)
    }
}

impl<const SIZE: usize> ByteAllocator for EarlyAllocator<SIZE> {
    fn alloc(
        &mut self,
        layout: core::alloc::Layout,
    ) -> allocator::AllocResult<core::ptr::NonNull<u8>> {
        // Calculate aligned start offset
        let aligned_b_pos = align_up(self.b_pos, layout.align());
        let new_b_pos = aligned_b_pos.checked_add(layout.size());
        
        let new_b_pos = match new_b_pos {
            Some(pos) => pos,
            None => return Err(allocator::AllocError::NoMemory),
        };

        // collision check
        if new_b_pos > self.p_pos {
            return Err(allocator::AllocError::NoMemory);
        }

        // update state
        self.b_pos = new_b_pos;
        self.b_count += 1;

        unsafe {
            let ptr = (self.arena.as_mut_ptr() as *mut u8).add(aligned_b_pos);
            Ok(core::ptr::NonNull::new_unchecked(ptr))
        }
    }

    fn dealloc(&mut self, pos: core::ptr::NonNull<u8>, layout: core::alloc::Layout) {
        if self.b_count == 0 {
            return;
        }
        
        self.b_count -= 1;
        // If b_count equals to 0, reset this area.
        if self.b_count == 0 {
            self.b_pos = self.b_start;
        }
    }

    fn total_bytes(&self) -> usize {
        self.p_end - self.b_start
    }

    fn used_bytes(&self) -> usize {
        (self.b_pos - self.b_start) + (self.p_end - self.p_pos)
    }

    fn available_bytes(&self) -> usize {
        self.p_pos.saturating_sub(self.b_pos)
    }
}

impl<const SIZE: usize> PageAllocator for EarlyAllocator<SIZE> {
    const PAGE_SIZE: usize = SIZE;

    fn alloc_pages(
        &mut self,
        num_pages: usize,
        align_pow2: usize,
    ) -> allocator::AllocResult<usize> {
        if num_pages == 0 {
            return Err(allocator::AllocError::NoMemory);
        }

        // Calculate total size
        let total_size = match num_pages.checked_mul(Self::PAGE_SIZE) {
            Some(s) => s,
            None => return Err(allocator::AllocError::NoMemory),
        };

        // Check alignment requirement
        let align = core::cmp::max(Self::PAGE_SIZE, align_pow2);

        let new_p_pos_unaligned = match self.p_pos.checked_sub(total_size) {
            Some(pos) => pos,
            None => return Err(allocator::AllocError::NoMemory), // 空间不足
        };

        // Algin down p_pos
        let  new_p_pos_aligned = align_down(new_p_pos_unaligned, align);

        // Collision check
        if new_p_pos_aligned < self.b_pos {
            return Err(allocator::AllocError::NoMemory);
        }

        self.p_pos = new_p_pos_aligned;
        
        let phys_addr = self.arena.as_ptr() as usize + new_p_pos_aligned;
        Ok(phys_addr)
    }

    fn dealloc_pages(&mut self, pos: usize, num_pages: usize) {
       //
    }

    fn total_pages(&self) -> usize {
        (self.p_end - self.b_start) / Self::PAGE_SIZE
    }

    fn used_pages(&self) -> usize {
        (self.p_end - self.p_pos) / Self::PAGE_SIZE
    }

    fn available_pages(&self) -> usize {
        self.p_pos.saturating_sub(self.b_pos) / Self::PAGE_SIZE
    }
}