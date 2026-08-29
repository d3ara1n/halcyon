//! 分级物理帧库存 host 测试。
//!
//! host 测试需显式 `--target aarch64-apple-darwin`（os/.cargo/config.toml
//! 的默认 target 指向 RISC-V）。

use std::{boxed::Box, vec};

use frame_pool::{
    AddRegionError, AllocAtError, ArenaMetadata, ExtentGeometry, FramePool, MAX_ARENAS,
    metadata_bytes,
};
use page_table::FrameNumber;

type Pool = FramePool<'static>;

fn pool(frame_capacity: usize) -> Pool {
    pool_with_arenas(frame_capacity, usize::BITS as usize * 2)
}

fn pool_with_arenas(frame_capacity: usize, arena_capacity: usize) -> Pool {
    let bytes = metadata_bytes(frame_capacity).unwrap();
    let metadata = Box::leak(vec![0; bytes].into_boxed_slice());
    let arenas = Box::leak(vec![ArenaMetadata::EMPTY; arena_capacity].into_boxed_slice());
    FramePool::new(metadata, arenas)
}

fn frame(number: usize) -> FrameNumber {
    FrameNumber(number)
}

fn add(pool: &mut Pool, start: usize, end: usize) {
    pool.add_region(frame(start), frame(end)).unwrap();
}

#[test]
fn exact_order_consume_and_return() {
    let mut pool = pool(16);
    add(&mut pool, 16, 32);

    assert_eq!(pool.free_frames(), 16);
    assert_eq!(pool.alloc_order(4), Some(frame(16)));
    assert_eq!(pool.free_frames(), 0);
    assert_eq!(pool.alloc_order(0), None);

    pool.dealloc(frame(16), 16);
    assert_eq!(pool.free_frames(), 16);
    assert_eq!(pool.alloc_order(4), Some(frame(16)));
}

#[test]
fn split_and_coalesce_restore_large_order() {
    let mut pool = pool(32);
    add(&mut pool, 0, 32);

    let a = pool.alloc_order(0).unwrap();
    let b = pool.alloc_order(0).unwrap();
    assert_eq!((a, b), (frame(0), frame(1)));
    assert_eq!(pool.alloc_order(5), None);

    pool.dealloc(a, 1);
    assert_eq!(pool.alloc_order(5), None);
    pool.dealloc(b, 1);
    assert_eq!(pool.alloc_order(5), Some(frame(0)));
}

#[test]
fn allocations_are_globally_order_aligned() {
    let mut pool = pool(32);
    add(&mut pool, 3, 35);

    let block = pool.alloc_order(4).unwrap();
    assert_eq!(block, frame(16));
    assert_eq!(block.0 % 16, 0);
    assert_eq!(pool.free_frames(), 16);
}

#[test]
fn fragmented_free_frames_do_not_fake_larger_order() {
    let mut pool = pool(16);
    add(&mut pool, 0, 16);
    let allocated: Vec<_> = (0..16).map(|_| pool.alloc_order(0).unwrap()).collect();

    for base in allocated.iter().step_by(2) {
        pool.dealloc(*base, 1);
    }
    assert_eq!(pool.free_frames(), 8);
    assert_eq!(pool.alloc_order(1), None);
    assert!(pool.alloc_order(0).is_some());
}

#[test]
fn extent_geometry_split_is_exact_and_non_overlapping() {
    let geometry = ExtentGeometry::new(frame(12), 8).unwrap();
    assert_eq!(geometry.base(), frame(12));
    assert_eq!(geometry.count(), 8);
    assert_eq!(geometry.end(), frame(20));
    assert_eq!(geometry.split_at(0), None);
    assert_eq!(geometry.split_at(8), None);

    let (left, right) = geometry.split_at(3).unwrap();
    assert_eq!(
        (left.base(), left.count(), left.end()),
        (frame(12), 3, frame(15))
    );
    assert_eq!(
        (right.base(), right.count(), right.end()),
        (frame(15), 5, frame(20))
    );
}

#[test]
fn alloc_largest_falls_back_without_rescanning_orders() {
    let mut pool = pool(16);
    add(&mut pool, 0, 16);
    let _whole = pool.alloc_order(4).unwrap();
    pool.dealloc(frame(0), 4);
    pool.dealloc(frame(8), 4);

    let (base, count) = pool.alloc_largest(16).unwrap();
    assert_eq!((base, count), (frame(0), 4));
    pool.dealloc(base, count);
    pool.dealloc(frame(4), 4);
    pool.dealloc(frame(12), 4);
    assert_eq!(pool.alloc_order(4), Some(frame(0)));
}

#[test]
fn alloc_at_is_exact_and_failure_is_atomic() {
    let mut pool = pool(16);
    add(&mut pool, 0, 16);

    pool.alloc_at(frame(6), 4).unwrap();
    assert_eq!(pool.free_frames(), 12);
    pool.alloc_at(frame(1), 2).unwrap();
    assert_eq!(pool.free_frames(), 10);

    assert_eq!(pool.alloc_at(frame(20), 2), Err(AllocAtError::Unavailable));
    assert_eq!(pool.alloc_at(frame(4), 4), Err(AllocAtError::Unavailable));
    assert_eq!(pool.alloc_at(frame(14), 4), Err(AllocAtError::Unavailable));
    assert_eq!(pool.free_frames(), 10);

    pool.dealloc(frame(6), 4);
    pool.dealloc(frame(1), 2);
    pool.alloc_at(frame(0), 16).unwrap();
    assert_eq!(pool.free_frames(), 0);
}

#[test]
fn alloc_at_crosses_canonical_arena_boundaries() {
    let mut pool = pool(16);
    add(&mut pool, 3, 19);

    pool.alloc_at(frame(5), 12).unwrap();
    assert_eq!(pool.free_frames(), 4);
    pool.dealloc(frame(5), 12);
    assert_eq!(pool.free_frames(), 16);
}

#[test]
fn managed_reservations_publish_later() {
    let mut pool = pool(16);
    pool.add_managed_region(frame(0), frame(16)).unwrap();
    pool.release_range(frame(0), frame(4)).unwrap();
    pool.release_range(frame(8), frame(16)).unwrap();

    assert_eq!(pool.free_frames(), 12);
    assert_eq!(pool.alloc_at(frame(4), 4), Err(AllocAtError::Unavailable));

    pool.release_range(frame(4), frame(8)).unwrap();
    assert_eq!(pool.free_frames(), 16);
    assert_eq!(pool.alloc_order(4), Some(frame(0)));
}

#[test]
fn multiple_managed_regions_never_bridge_physical_gaps() {
    let mut pool = pool(16);
    add(&mut pool, 0, 8);
    add(&mut pool, 32, 40);

    assert_eq!(pool.free_frames(), 16);
    assert_eq!(pool.alloc_order(4), None);
    assert!(pool.alloc_order(3).is_some());
    assert!(pool.alloc_order(3).is_some());
    assert_eq!(pool.free_frames(), 0);
}

#[test]
fn metadata_and_arena_limits_fail_before_mutation() {
    let mut short = pool(7);
    assert_eq!(
        short.add_managed_region(frame(0), frame(8)),
        Err(AddRegionError::MetadataExhausted)
    );
    assert_eq!(short.arena_count(), 0);

    let mut overlap = pool(16);
    overlap.add_managed_region(frame(0), frame(8)).unwrap();
    assert_eq!(
        overlap.add_managed_region(frame(4), frame(12)),
        Err(AddRegionError::Overlap)
    );
    assert_eq!(overlap.arena_count(), 1);
    assert!(overlap.arena_count() <= MAX_ARENAS);

    let mut arena_short = pool_with_arenas(32, 1);
    assert_eq!(
        arena_short.add_managed_region(frame(3), frame(35)),
        Err(AddRegionError::ArenaLimit)
    );
    assert_eq!(arena_short.arena_count(), 0);
}

#[test]
fn canonical_arena_count_has_address_width_bound() {
    let mut pool = pool(10_000);
    pool.add_managed_region(frame(3), frame(10_003)).unwrap();
    assert!(pool.arena_count() <= usize::BITS as usize * 2);
}

#[test]
fn stress_conserves_frames_and_recoalesces() {
    let mut pool = pool(256);
    add(&mut pool, 0, 256);
    let total = 256;
    let mut held: Vec<(FrameNumber, usize)> = Vec::new();
    let mut rng: u64 = 0x243F_6A88_85A3_08D3;

    for round in 0..2000 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        if (rng >> 33) % 4 < 2 {
            let max = ((rng >> 40) % 16 + 1) as usize;
            if let Some(block) = pool.alloc_largest(max) {
                held.push(block);
            }
        } else if !held.is_empty() {
            let index = (rng as usize) % held.len();
            let (base, count) = held.swap_remove(index);
            pool.dealloc(base, count);
        }
        let held_frames: usize = held.iter().map(|(_, count)| *count).sum();
        assert_eq!(
            pool.free_frames() + held_frames,
            total,
            "frame count not conserved at round {round}"
        );
    }

    for (base, count) in held {
        pool.dealloc(base, count);
    }
    assert_eq!(pool.free_frames(), total);
    assert_eq!(pool.alloc_order(8), Some(frame(0)));
}

#[test]
#[should_panic(expected = "zero-frame deallocation")]
fn zero_count_dealloc_panics() {
    let mut pool = pool(4);
    add(&mut pool, 0, 4);
    pool.dealloc(frame(0), 0);
}

#[test]
#[should_panic(expected = "overlaps free inventory")]
fn double_free_panics() {
    let mut pool = pool(8);
    add(&mut pool, 0, 8);
    let block = pool.alloc_order(2).unwrap();
    pool.dealloc(block, 4);
    pool.dealloc(block, 4);
}
