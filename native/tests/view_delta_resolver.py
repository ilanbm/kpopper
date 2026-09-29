#!/usr/bin/env python3
"""Independent executable golden checker; no model or network.

Preserve JSON numeric tokens while hashing. In particular, do not round arbitrary
integers or convert -0.0/decimal metadata through a binary float and reserialize.
This demonstrates the projection and published positive/negative fixtures. It
is not a production receipt client or a replacement for all Rust V3 validation.
Run: python3 native/tests/view_delta_resolver.py
"""
import copy
import hashlib
import json
import datetime
import math
import re
from pathlib import Path


class NumberToken(str):
    pass


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def loads(value):
    return json.loads(value, parse_float=NumberToken, object_pairs_hook=pairs)


def dumps(value, sort=True):
    if isinstance(value, NumberToken):
        return str(value)
    if isinstance(value, dict):
        keys = sorted(value) if sort else value
        return "{" + ",".join(dumps(k) + ":" + dumps(value[k], sort) for k in keys) + "}"
    if isinstance(value, list):
        return "[" + ",".join(dumps(v, sort) for v in value) + "]"
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False)


def digest(value):
    return hashlib.sha256(dumps(value).encode()).hexdigest()


def ensure(condition, message):
    if not condition:
        raise ValueError(message)


def validate_hash(value):
    ensure(type(value) is str and re.fullmatch(r"[0-9a-f]{64}", value), "invalid SHA-256")


def validate_typed(value):
    pending, visits = [(value, 0)], 0
    while pending:
        item, depth = pending.pop()
        visits += 1
        ensure(depth <= 128 and visits <= 100000, "typed value limit")
        ensure(isinstance(item, list) and len(item) >= 1 and type(item[0]) is str, "invalid typed value")
        tag = item[0]
        if tag == "null":
            ensure(len(item) == 1, "invalid typed null")
            continue
        ensure(len(item) == 2, "invalid typed arity")
        v = item[1]
        if tag == "bool":
            ensure(type(v) is bool, "invalid typed bool")
        elif tag in ("text", "int", "float", "date", "datetime"):
            ensure(type(v) is str, "invalid typed scalar")
            if tag == "int":
                ensure(re.fullmatch(r"0|-?[1-9][0-9]*", v), "invalid typed int")
            elif tag == "float":
                number = float.fromhex(v)
                ensure(math.isfinite(number) and number.hex() == v, "invalid typed float")
            elif tag == "date":
                ensure(datetime.date.fromisoformat(v).isoformat() == v, "invalid typed date")
            elif tag == "datetime":
                ensure(datetime.datetime.fromisoformat(v).isoformat() == v, "invalid typed datetime")
        elif tag == "list":
            ensure(isinstance(v, list), "invalid typed list")
            pending.extend((child, depth + 1) for child in v)
        elif tag == "map":
            ensure(isinstance(v, list), "invalid typed map")
            keys = []
            for pair in v:
                ensure(isinstance(pair, list) and len(pair) == 2 and type(pair[0]) is str, "invalid typed pair")
                keys.append(pair[0])
                pending.append((pair[1], depth + 1))
            ensure(keys == sorted(set(keys)), "noncanonical typed map")
        else:
            raise ValueError("unknown typed tag")


def slot(alias):
    ensure(isinstance(alias, str) and alias[1:].isdigit(), "bad alias")
    ensure(str(int(alias[1:])) == alias[1:], "noncanonical alias")
    return [alias[0], int(alias[1:])]


def translate(packet, dictionary, forward):
    def ref(value, kind=None):
        if forward:
            identity = dictionary[value]
            ensure(kind is None or identity["kind"] == kind, "wrong reference kind")
            return copy.deepcopy(identity)
        ensure(kind is None or value["kind"] == kind, "wrong reference kind")
        matches = [key for key, entry in dictionary.items() if entry == value]
        ensure(len(matches) == 1, "ambiguous original identity")
        return matches[0]

    def pool(value):
        return slot(value) if forward else f"{value[0]}{value[1]}"

    def edge(value, qualified):
        if forward:
            prefix = f"edgeset:{packet['view_id']}:"
            if qualified:
                ensure(value.startswith(prefix), "stale edge set")
                value = value[len(prefix):]
            return slot(value)[1]
        return (f"edgeset:{packet['view_id']}:" if qualified else "") + f"e{value}"

    pooled = set()
    manifest = packet.get("attribute_compaction")
    if manifest is not None:
        entries = list(manifest["nodes"].items()) if forward else manifest["nodes"]
        result = [] if forward else {}
        for key, rule in entries:
            if "status" in rule:
                rule["status"] = pool(rule["status"])
                pooled.add(dictionary[key]["original"] if forward else key["original"])
            key = ref(key, "node")
            if forward:
                result.append([key, rule])
            else:
                ensure(key not in result, "duplicate compaction rule")
                result[key] = rule
        manifest["nodes"] = result
        for field in ("compacted_nodes", "status_pool_nodes", "status_text_nodes", "value_text_nodes"):
            manifest[field] = [ref(value, "node") for value in manifest[field]]
    for row in packet["nodes"]:
        original = dictionary[row[0]]["original"] if forward else row[0]["original"]
        if original in pooled:
            row[2]["status_ref"] = pool(row[2]["status_ref"])
        row[0] = ref(row[0], "node")
    for row in packet["groups"]:
        ensure(type(row[1]) is int and row[1] >= 0, "invalid group count")
        row[0] = ref(row[0], "group")
    for row in packet["links"]:
        ensure(type(row[3]) is int and row[3] >= 0, "invalid link count")
        row[0], row[2], row[4] = ref(row[0]), ref(row[2]), edge(row[4], False)
    for row in packet["expanded_edges"]:
        ensure(isinstance(row[1], list) and all(isinstance(e, dict) for e in row[1]), "invalid original edges")
        row[0] = edge(row[0], True)
    for row in packet["navigation_facets"]["groups"]:
        ensure(type(row[1]) is int and row[1] >= 0, "invalid navigation count")
        row[0] = ref(row[0], "group")
    for row in packet["navigation_facets"]["nodes"]:
        row[0], row[1] = ref(row[0], "node"), [ref(value, "group") for value in row[1]]
    if pooled and "shared_assessments" in packet:
        assessments = packet["shared_assessments"]
        packet["shared_assessments"] = (
            [[slot(key)[1], value] for key, value in sorted(assessments.items())]
            if forward else {f"a{index}": value for index, value in assessments})


def project(packet):
    normalized = copy.deepcopy(packet)
    dictionary = normalized["dictionary"]
    translate(normalized, dictionary, True)
    normalized["dictionary"] = [{"slot": slot(key), "identity": entry}
                                for key, entry in sorted(dictionary.items())]
    nodes, evidence, order = {}, {}, []
    for identity, body, attributes in normalized.pop("nodes"):
        validate_typed(body)
        ensure(identity["kind"] == "node" and isinstance(attributes, dict), "invalid node row")
        original = identity["original"]
        ensure(original not in nodes, "duplicate source row")
        nodes[original] = {"source_id": original, "body_sha256": digest(body), "attributes": attributes}
        evidence[original] = {"source_id": original, "body_sha256": digest(body), "body": body}
        order.append(original)
    return {"metadata": normalized, "nodes": nodes, "order": order}, evidence


def restore(projection, evidence):
    result = copy.deepcopy(projection["metadata"])
    dictionary = {}
    for binding in result["dictionary"]:
        prefix, ordinal = binding["slot"]
        key = f"{prefix}{ordinal}"
        ensure(key not in dictionary, "duplicate slot")
        dictionary[key] = binding["identity"]
    ensure(len(set(projection["order"])) == len(projection["order"]), "duplicate row order")
    ensure(set(projection["order"]) == set(projection["nodes"]), "incomplete row order")
    result["nodes"] = []
    for original in projection["order"]:
        node = projection["nodes"][original]
        body = evidence[original]
        ensure(node["body_sha256"] == body["body_sha256"], "body mismatch")
        identities = [entry for entry in dictionary.values()
                      if entry["kind"] == "node" and entry["original"] == original]
        ensure(len(identities) == 1, "ambiguous identity")
        result["nodes"].append(copy.deepcopy([identities[0], body["body"], node["attributes"]]))
    translate(result, dictionary, False)
    result["dictionary"] = dictionary
    ensure(project(result)[0] == projection, "noncanonical projection")
    return result


def apply(packet, evidence, delta):
    for field in ("project_sha256", "base_sha256", "target_sha256", "base_evidence_sha256", "target_evidence_sha256"):
        validate_hash(delta[field])
    ensure(delta["schema"] == "kpopper.selected-view-delta/v1", "unsupported schema")
    ensure(packet["revision"] == delta["revision"] and packet["scope"] == delta["scope"], "wrong snapshot")
    ensure(digest([packet["project"], packet["project_identity"]]) == delta["project_sha256"], "wrong project")
    ensure(digest(packet) == delta["base_sha256"], "wrong base")
    ensure(digest(evidence) == delta["base_evidence_sha256"], "wrong received evidence")
    projection, _ = project(packet)
    evidence = copy.deepcopy(evidence)
    for collection in (delta["evidence_additions"], delta["focus"]["upsert"]):
        ids = [row["source_id"] for row in collection]
        ensure(ids == sorted(set(ids)), "noncanonical rows")
    for collection in (delta["focus"]["fold"], delta["metadata"]["unset"]):
        ensure(collection == sorted(set(collection)), "noncanonical removals")
    for item in delta["evidence_additions"]:
        validate_typed(item["body"])
        validate_hash(item["body_sha256"])
        original = item["source_id"]
        ensure(original not in evidence, "evidence is additive")
        ensure(digest(item["body"]) == item["body_sha256"], "invalid body digest")
        ensure(any(n["source_id"] == original and n["body_sha256"] == item["body_sha256"]
                   for n in delta["focus"]["upsert"]), "unselected evidence")
        evidence[original] = copy.deepcopy(item)
    for key in delta["metadata"]["unset"]:
        ensure(key not in delta["metadata"]["set"], "contradictory metadata")
        del projection["metadata"][key]
    ensure("nodes" not in delta["metadata"]["set"], "nodes belong to focus")
    projection["metadata"].update(copy.deepcopy(delta["metadata"]["set"]))
    for original in delta["focus"]["fold"]:
        ensure(not any(n["source_id"] == original for n in delta["focus"]["upsert"]), "contradictory focus")
        del projection["nodes"][original]
    for node in delta["focus"]["upsert"]:
        projection["nodes"][node["source_id"]] = copy.deepcopy(node)
    projection["order"] = delta["focus"]["order"]
    target = restore(projection, evidence)
    ensure(all(target[k] == packet[k] for k in ("revision", "scope", "project", "project_identity")), "changed snapshot")
    ensure(digest(target) == delta["target_sha256"], "wrong target digest")
    ensure(digest(evidence) == delta["target_evidence_sha256"], "wrong cumulative digest")
    return target, evidence


def decode_base_text(text):
    lines = text.splitlines()
    ensure(lines.pop(0) == "# Checked graph view (kpopper.view-codec/v4; kpopper.canonical-graph-view/v3)", "unsupported base")
    packet, section, count = {}, None, 0
    tags = {"nodes": "node", "groups": "group", "links": "link", "expanded_edges": "edge"}
    for line in lines:
        if not line or line.startswith("## "):
            continue
        if line.startswith("@"):
            if section:
                ensure(len(packet[section]) == count, "truncated section")
            kind, _, tail = line.partition(" ")
            key, _, data = tail.partition(" ")
            key = loads(key)
            ensure(key not in packet, "duplicate section")
            section = None
            if kind == "@field":
                packet[key] = loads(data)
            else:
                ensure(kind == "@rows" and key in tags, "unknown section")
                section, count = key, int(data)
                packet[key] = []
        else:
            ensure(section in tags, "row without section")
            tag, _, data = line.partition(" ")
            ensure(tag == tags[section] and len(packet[section]) < count, "wrong row")
            packet[section].append(loads(data))
    if section:
        ensure(len(packet[section]) == count, "truncated final section")
    return packet


def check(path):
    golden = loads(path.read_text())
    packet = decode_base_text(golden["base_text"])
    ensure(packet == golden["base"], "text base differs")
    lines = golden["delta_text"].splitlines()
    ensure(len(lines) == 3 and lines[0] == "# Checked graph delta (kpopper.selected-view-delta/v1)", "bad delta frame")
    delta = loads(lines[2])
    ensure(delta == golden["delta"], "text delta differs")
    _, received = project(packet)
    target, received = apply(packet, received, delta)
    ensure(target == golden["target"], "exact V3 target reconstruction failed")
    ensure(received == golden["received"], "cumulative evidence differs")
    for original, body in golden["resolution"].items():
        ensure(received.get(original, {}).get("body") == body, f"wrong original-ID resolution: {original}")
    for field in ("base_sha256", "target_sha256", "base_evidence_sha256", "target_evidence_sha256", "project_sha256"):
        changed = copy.deepcopy(delta)
        changed[field] = "0" * 64
        try:
            apply(packet, project(packet)[1], changed)
        except ValueError:
            pass
        else:
            raise AssertionError(f"accepted altered {field}")
    if "isolate.null" in received:
        forged, forged_target, forged_evidence = map(copy.deepcopy, (delta, target, received))
        bad_body = ["text", 5]
        bad_hash = digest(bad_body)
        for row in forged["evidence_additions"]:
            if row["source_id"] == "isolate.null":
                row["body"], row["body_sha256"] = bad_body, bad_hash
                forged_evidence["isolate.null"] = copy.deepcopy(row)
        for row in forged["focus"]["upsert"]:
            if row["source_id"] == "isolate.null":
                row["body_sha256"] = bad_hash
        for row in forged_target["nodes"]:
            if forged_target["dictionary"][row[0]]["original"] == "isolate.null":
                row[1] = bad_body
        forged["target_sha256"] = digest(forged_target)
        forged["target_evidence_sha256"] = digest(forged_evidence)
        try:
            apply(packet, project(packet)[1], forged)
        except ValueError as error:
            ensure(str(error) == "invalid typed scalar", "forgery reached the wrong guard")
        else:
            raise AssertionError("accepted self-consistent invalid typed body")
    print("Rust/Python exact roundtrip, base-text + delta-text resolver, alias rebinding, additive bodies, and digest mutations: passed")
    print(dumps(golden["sizes"]))


if __name__ == "__main__":
    for filename in ("view-delta-v1.json", "view-delta-v1-reuse.json"):
        check(Path(__file__).with_name("fixtures") / filename)
