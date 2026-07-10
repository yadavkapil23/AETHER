"""Timeout handler using asyncio.wait_for."""

import asyncio
from typing import Awaitable, TypeVar, Callable

T = TypeVar('T')

class TimeoutError(asyncio.TimeoutError):
    """Custom timeout error for clarity."""
    pass

async def with_timeout(coro: Awaitable[T], timeout_seconds: float) -> T:
    """
    Execute a coroutine with a timeout.

    Args:
        coro: The coroutine to run.
        timeout_seconds: Timeout in seconds (float allowed).

    Returns:
        The result of the coroutine.

    Raises:
        TimeoutError: If the coroutine does not complete within the timeout.
    """
    try:
        return await asyncio.wait_for(coro, timeout=timeout_seconds)
    except asyncio.TimeoutError as exc:
        raise TimeoutError(f"Operation timed out after {timeout_seconds}s") from exc

def timeout_decorator(timeout_seconds: float):
    """
    Decorator to apply a timeout to an async function.

    Usage:
        @timeout_decorator(5.0)
        async def my_func(...):
            ...
    """
    def decorator(func: Callable[..., Awaitable[T]]) -> Callable[..., Awaitable[T]]:
        async def wrapper(*args, **kwargs) -> T:
            return await with_timeout(func(*args, **kwargs), timeout_seconds)
        return wrapper
    return decorator