"""Retry handler with exponential backoff and jitter."""

import random
import asyncio
from typing import Awaitable, Callable, TypeVar, Tuple, Type

T = TypeVar('T')

class RetryError(Exception):
    """Raised when all retry attempts are exhausted."""
    def __init__(self, last_exception: Exception, attempts: int):
        super().__init__(f"Operation failed after {attempts} attempts")
        self.last_exception = last_exception
        self.attempts = attempts

async def retry(
    func: Callable[..., Awaitable[T]],
    *args,
    max_attempts: int = 5,
    initial_backoff: float = 0.1,  # seconds
    max_backoff: float = 5.0,
    backoff_multiplier: float = 2.0,
    jitter: bool = True,
    retry_exceptions: Tuple[Type[BaseException], ...] = (Exception,),
    **kwargs,
) -> T:
    """
    Execute a coroutine with retry logic.

    Args:
        func: Async callable to retry.
        *args, **kwargs: Arguments passed to func.
        max_attempts: Maximum number of attempts (including first try).
        initial_backoff: Initial delay in seconds.
        max_backoff: Maximum delay between attempts.
        backoff_multiplier: Multiplier for exponential backoff.
        jitter: If True, adds random jitter to prevent thundering herd.
        retry_exceptions: Tuple of exception types to catch and retry.
                          Any other exception will be raised immediately.

    Returns:
        The result of the function call.

    Raises:
        RetryError: If all attempts fail.
        Any exception not in ``retry_exceptions`` is propagated immediately.
    """
    attempt = 0
    backoff = initial_backoff
    while True:
        try:
            return await func(*args, **kwargs)
        except retry_exceptions as e:
            attempt += 1
            if attempt >= max_attempts:
                raise RetryError(e, attempt) from e
            # Compute delay with exponential backoff
            delay = min(backoff * (backoff_multiplier ** (attempt - 1)), max_backoff)
            if jitter:
                # Full jitter: random delay between 0 and `delay`
                delay = random.uniform(0, delay)
            await asyncio.sleep(delay)
        except Exception as e:
            # Non-retriable exception
            raise