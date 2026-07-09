from aether.scheduler import KVCacheAllocator


def test_allocator_allocates_and_deallocates_blocks():
    allocator = KVCacheAllocator(total_bytes=1024 * 1024, block_size=16 * 1024)
    blocks = allocator.allocate("req-1", 10)

    assert len(blocks) == 10
    assert allocator.stats().allocated_blocks == 10

    released = allocator.deallocate(blocks)

    assert released == 10
    assert allocator.stats().allocated_blocks == 0


def test_allocator_rejects_oversized_request():
    allocator = KVCacheAllocator(total_bytes=1024, block_size=256)

    try:
        allocator.allocate("req-1", 5)
    except RuntimeError as exc:
        assert "insufficient free blocks" in str(exc)
    else:
        raise AssertionError("expected allocation to fail")
