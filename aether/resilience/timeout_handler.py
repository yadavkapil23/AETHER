"""Timeout handler using asyncio.wait_for."""

import asyncio
from typing import Awaitable, Callable, TypeVar

T = TypeVar("T")


class TimeoutError(asyncio.TimeoutError):
    """Raised when an operation exceeds the allowed timeout."""


async def with_timeout(coro: Awaitable[T], timeout_seconds: float) -> T:
    """
    Await ``coro`` with a timeout.

    Raises
    ------
    TimeoutError
        If the coroutine does not complete within ``timeout_seconds``.
    """
    try:
        return await asyncio.wait_for(coro, timeout=timeout_seconds)
    except asyncio.TimeoutError as exc:
        raise TimeoutError(f"operation timed out after {timeout_seconds}s") from exc


def timeout_decorator(timeout_seconds: float):
    """
    Decorator to apply a timeout to an async function.

    Example
    -------
    @timeout_decorator(2.5)
    async def my_func(...):
        ...
    """

    def decorator(func: Callable[..., Awaitable[T]]) -> Callable[..., Awaitable[T]]:
        async def wrapper(*args, **kwargs) -> T:
            return await with_timeout(func(*args, **kwargs), timeout_seconds)

        return wrapper

    return decorator
