"""Generate comparison fixtures from pySigma's actual parsed condition tree.
Requires the isolated, pinned pySigma environment; it is not a runtime dependency.
The small event interpreter is explicit and does not execute log content.
"""
import base64
import importlib.metadata
import ipaddress
import json
import re
import uuid
from pathlib import Path
import yaml
from sigma.rule import SigmaRule
from sigma.conditions import ConditionAND, ConditionOR, ConditionNOT, ConditionFieldEqualsValueExpression
from sigma.types import SigmaString, SigmaCasedString, SigmaRegularExpression, SigmaNumber, SigmaCompareExpression, SigmaExists, SigmaNull, SigmaCIDRExpression, SigmaFieldReference, SpecialChars, SigmaExpansion

REVISION = "f81e4f5ace2f444f76c5de03df8e0f181f85f6cc"
assert importlib.metadata.version("pySigma") == "1.5.1"
MISSING = object()

def match_value(value, actual, fields):
    if isinstance(value, SigmaExpansion): return any(match_value(v, actual, fields) for v in value.values)
    if isinstance(value, SigmaExists): return (actual is not MISSING) == value.exists
    if isinstance(value, SigmaNull): return actual is MISSING or actual is None
    if actual is MISSING or actual is None: return False
    if isinstance(value, SigmaString):
        pattern = "".join(".*" if part == SpecialChars.WILDCARD_MULTI else "." if part == SpecialChars.WILDCARD_SINGLE else re.escape(part) for part in value.s)
        return re.fullmatch(pattern, str(actual), re.DOTALL | (0 if isinstance(value, SigmaCasedString) else re.IGNORECASE)) is not None
    if isinstance(value, SigmaRegularExpression):
        flags = sum(value.sigma_to_python_flags[flag] for flag in value.flags)
        return re.search(str(value.regexp), str(actual), flags) is not None
    if isinstance(value, SigmaNumber): return actual == value.number
    if isinstance(value, SigmaCompareExpression):
        op = value.op.name
        expected = value.number.number
        return {"GT": actual > expected, "GTE": actual >= expected, "LT": actual < expected, "LTE": actual <= expected, "NEQ": actual != expected}[op]
    if isinstance(value, SigmaCIDRExpression):
        try: return ipaddress.ip_address(actual) in ipaddress.ip_network(value.cidr)
        except ValueError: return False
    if isinstance(value, SigmaFieldReference):
        other = fields.get(value.field, MISSING)
        return other is not MISSING and other is not None and str(actual).lower() == str(other).lower()
    raise TypeError(type(value).__name__)

def evaluate(node, fields):
    if isinstance(node, ConditionAND): return all(evaluate(arg, fields) for arg in node.args)
    if isinstance(node, ConditionOR): return any(evaluate(arg, fields) for arg in node.args)
    if isinstance(node, ConditionNOT): return not evaluate(node.args[0], fields)
    if isinstance(node, ConditionFieldEqualsValueExpression): return match_value(node.value, fields.get(node.field, MISSING), fields)
    raise TypeError(type(node).__name__)

specs = [
    ("contains_unicode", {"x|contains": "ação"}, [{"x": "AÇÃO approved"}, {"x": "action"}, {}]),
    ("startswith", {"x|startswith": "abc"}, [{"x": "ABCdef"}, {"x": "xabc"}, {}]),
    ("endswith", {"x|endswith": ".exe"}, [{"x": "cmd.EXE"}, {"x": "exe.cmd"}, {}]),
    ("contains_all", {"x|contains|all": ["one", "two"]}, [{"x": "two one"}, {"x": "one"}, {}]),
    ("cased", {"x|cased": "Hello"}, [{"x": "Hello"}, {"x": "hello"}, {}]),
    ("regex_default", {"x|re": "^Hello$"}, [{"x": "Hello"}, {"x": "hello"}, {}]),
    ("regex_i", {"x|re|i": "^Hello$"}, [{"x": "Hello"}, {"x": "hello"}, {"x": "xhello"}]),
    ("regex_m", {"x|re|m": "^Hello$"}, [{"x": "one\nHello\ntwo"}, {"x": "hello"}]),
    ("regex_s", {"x|re|s": "a.b"}, [{"x": "a\nb"}, {"x": "axb"}, {"x": "ac"}]),
    ("exists", {"x|exists": True}, [{"x": None}, {"x": ""}, {}, {"x": 0}]),
    ("not_exists", {"x|exists": False}, [{"x": None}, {"x": ""}, {}, {"x": 0}]),
    ("null", {"x": None}, [{"x": None}, {}, {"x": ""}, {"x": "null"}]),
    ("numeric", {"x|gte": 10}, [{"x": 10}, {"x": 9}, {"x": 11}, {}]),
    ("numeric_neq", {"x|neq": 10}, [{"x": 10}, {"x": 9}, {}]),
    ("cidr", {"x|cidr": "192.0.2.0/24"}, [{"x": "192.0.2.8"}, {"x": "192.0.3.8"}, {}]),
    ("fieldref", {"x|fieldref": "y"}, [{"x": "one", "y": "ONE"}, {"x": "one", "y": "two"}, {"x": "one"}]),
    ("windash", {"x|windash|contains": "-test -file-name"}, [{"x": "tool /test –file-name x"}, {"x": "tool -test -file-name"}, {"x": "-test -file/name"}]),
    ("base64", {"x|base64|cased": "hello"}, [{"x": base64.b64encode(b"hello").decode()}, {"x": "bogus"}, {}]),
    ("wide_base64", {"x|wide|base64|cased": "hello"}, [{"x": base64.b64encode("hello".encode("utf-16le")).decode()}, {"x": base64.b64encode(b"hello").decode()}, {}]),
    ("base64offset", {"x|base64offset|contains|cased": "hello"}, [{"x": base64.b64encode(prefix + b"hello" + b"suffix").decode()} for prefix in [b"", b"x", b"xy"]] + [{"x": "bogus"}]),
]
cases = []
for name, selection, events in specs:
    doc = {"title": name, "id": str(uuid.uuid5(uuid.NAMESPACE_DNS, f"loginsight-reference-{name}")), "logsource": {"category": "application"}, "detection": {"selection": selection, "condition": "selection"}}
    text = yaml.safe_dump(doc, sort_keys=False, allow_unicode=True)
    rule = SigmaRule.from_yaml(text)
    tree = rule.detection.parsed_condition[0].parsed
    case = {"id": name, "yaml": text, "events": events, "expected": [evaluate(tree, event) for event in events], "upstream_tree": str(tree)}
    required = [key.split('|')[0] for key in selection if 'neq' in key.split('|')[1:]]
    if required:
        # Keep the upstream results unchanged. The backend's documented guard
        # is a declared difference, not a silently rewritten reference answer.
        case['backend_policy'] = 'Log Insight neq requires the selected field present; a missing field is not an observed unequal value. The pySigma NOT tree alone has no existence guard.'
        case['backend_expected'] = [expected and all(field in event for field in required) for expected, event in zip(case['expected'], events)]
    cases.append(case)
document = {"version": "sigma-reference-1", "pySigma_version": "1.5.1", "upstream_revision": REVISION, "source": f"https://github.com/SigmaHQ/pySigma/tree/{REVISION}", "scope": "Parsed modifier and boolean semantics; explicit local event interpreter; not external execution of backend queries or temporal correlations", "cases": cases}
target = Path(__file__).resolve().parents[2] / "src-tauri/tests/fixtures/security/sigma-reference.json"
target.write_text(json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(json.dumps({"cases": len(cases), "events": sum(len(c['events']) for c in cases), "revision": REVISION}))
