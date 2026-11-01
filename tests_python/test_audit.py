from aether.audit import AuditTrail, new_audit_event


def test_audit_trail_verifies_hash_chain():
    trail = AuditTrail()
    trail.append(new_audit_event("req-1", "TOKEN_GENERATED", "hello"))
    trail.append(new_audit_event("req-1", "TOKEN_GENERATED", "world"))

    assert trail.current_hash
    assert trail.verify()
