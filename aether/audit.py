import json
import time
from dataclasses import asdict, dataclass

try:
    from blake3 import blake3
except ImportError:  # pragma: no cover
    import hashlib

    def blake3(data: bytes):
        return hashlib.blake2s(data)


@dataclass(frozen=True)
class AuditEvent:
    event_id: str
    request_id: str
    event_type: str
    payload: str
    timestamp_ns: int


class AuditTrail:
    def __init__(self) -> None:
        self._entries: list[tuple[AuditEvent, str, str]] = []
        self._current_hash = "0" * 64

    @property
    def current_hash(self) -> str:
        return self._current_hash

    def append(self, event: AuditEvent) -> str:
        body = json.dumps(asdict(event), sort_keys=True, separators=(",", ":")).encode()
        event_hash = blake3(body).hexdigest()
        chained = blake3(f"{self._current_hash}:{event_hash}".encode()).hexdigest()
        self._entries.append((event, event_hash, chained))
        self._current_hash = chained
        return chained

    def verify(self) -> bool:
        current = "0" * 64
        for event, event_hash, chained_hash in self._entries:
            body = json.dumps(asdict(event), sort_keys=True, separators=(",", ":")).encode()
            if blake3(body).hexdigest() != event_hash:
                return False
            current = blake3(f"{current}:{event_hash}".encode()).hexdigest()
            if current != chained_hash:
                return False
        return current == self._current_hash


def new_audit_event(request_id: str, event_type: str, payload: str) -> AuditEvent:
    return AuditEvent(
        event_id=f"evt-{time.time_ns()}",
        request_id=request_id,
        event_type=event_type,
        payload=payload,
        timestamp_ns=time.time_ns(),
    )

