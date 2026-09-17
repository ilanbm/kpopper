"""Offer changed followup readiness during a session; never execute or schedule work."""
import json
import sys

try:
    from . import followups as F
except ImportError:
    import followups as F


def handle(payload):
    # agent_type also names a main session started with --agent.
    if not isinstance(payload, dict) or payload.get("agent_id"):
        return ""
    owner = payload.get("session_id")
    if not isinstance(owner, str) or not owner or len(owner) > 200:
        return ""
    store = F.Store(payload.get("cwd"))
    if not store.path.exists() and not store.registered:
        return ""
    text = F.summary(store.location)
    fingerprint = F.digest(text)
    # Delivery receipts must not rotate the last product-state rollback point.
    with store.lock():
        path = store.root / "delivery.yaml"
        delivery = F.read_yaml(path) if path.exists() else []
        if not isinstance(delivery, list):
            raise F.Refused("Invalid followup delivery receipts")
        previous = next((row for row in delivery if row["session"] == owner), None)
        if previous and previous["fingerprint"] == fingerprint:
            return ""
        F.atomic(path, [row for row in delivery if row["session"] != owner][-127:] + [
            {"session": owner, "fingerprint": fingerprint}])
    return text


def main():
    try:
        text = handle(json.load(sys.stdin))
        if text:
            print(json.dumps({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": text}}, ensure_ascii=False))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("kpopper followup check unavailable: " + str(error), file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
