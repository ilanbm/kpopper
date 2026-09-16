"""Explicit conversion to structured expressions, with a checked migration preview."""
import argparse
import copy
import json
from pathlib import Path
import tempfile

try:
    from . import expressions as E, ingestion as I
except ImportError:
    import expressions as E
    import ingestion as I

P = I.P


def migrate(record=None, apply=False, readable=False):
    rec, project, policy = I._routed_record(record)
    with P._locked(str(rec), project=project):
        if project.config() != policy:
            raise ValueError('project mode or record destination changed; retry migration')
        doc, ids, judgments, fields, raw = I._record_world(rec)
        before = rec.read_bytes()
        changes, skipped, bodies = [], [], {}
        for nid, body in P.bodies(doc).items():
            if nid not in ids or not isinstance(body, dict):
                continue
            candidates = []
            if nid in judgments:
                candidates.append((fields["predicate"], True))
            else:
                candidates.append(("rule", False))
                if "rule" not in body and isinstance(body.get("v"), str) and P.rule_refs(body, ids):
                    candidates.append(("v", False))
            for field, predicate in candidates:
                value = body.get(field)
                old_tree = readable and isinstance(value, dict) and 'expr' not in value
                if not old_tree and (not isinstance(value, str) or not value.strip()):
                    continue
                try:
                    if old_tree:
                        converted = E.lower(value, predicate)
                    else:
                        comparison = P.CMP.match(value) if predicate else None
                        converted = E.convert_authored(value, predicate=predicate,
                            legacy_rhs=comparison.group(3) if comparison else None)
                    unknown = set(E.refs(converted)) - ids
                    if unknown:
                        raise ValueError("unknown or ambiguous bare names: " + ", ".join(sorted(unknown)))
                    if predicate and set(E.refs(converted)) - set(judgments[nid]["deps"]):
                        raise ValueError("predicate references are not declared in rests_on")
                    if readable:
                        converted = E.readable(converted, predicate)
                except (ValueError, SyntaxError, RecursionError) as error:
                    skipped.append({"id": nid, "field": field, "reason": str(error)})
                    continue
                target = "rule" if field == "v" else field
                replacement = bodies.setdefault(nid, copy.deepcopy(body))
                if target != field:
                    del replacement[field]
                replacement[target] = converted
                changes.append({"id": nid, "field": field, "target": target, "before": value, "after": converted})
        answer = {"record": str(rec), "before_sha256": I._sha(before), "changes": changes,
                  "skipped": skipped, "applied": False, "problems": [], "fired": []}
        if not changes:
            return answer
        with tempfile.TemporaryDirectory(prefix="kpopper-expression-migration-") as directory:
            shadow = Path(directory) / rec.name
            # The editor matches logical lines; retain the record's newline style
            # when writing bytes so Windows does not translate them a second time.
            source = before.decode("utf-8")
            newline = "\r\n" if source.count("\r\n") > source.count("\n") / 2 else "\n"
            lines = source.replace("\r\n", "\n").split("\n")
            try:
                from .sameness import _set_fields
            except ImportError:
                from sameness import _set_fields
            original_bodies = P.bodies(doc)
            for nid, replacement in bodies.items():
                _set_fields(lines, nid, replacement, original_bodies[nid])
            shadow.write_bytes(newline.join(lines).encode("utf-8"))
            brief = P._brief_beside(str(rec))
            if brief:
                target_brief = Path(P.layout(shadow)["view"])
                target_brief.parent.mkdir(parents=True, exist_ok=True)
                target_brief.write_bytes(Path(brief).read_bytes())
            baseline = set(P.check_lines([str(rec)])[0])
            errors = P.check_lines([str(shadow)])[0]
            discovered = set()
            _, after_ids, after_judgments, _, after_raw = I._record_world(shadow)
            for nid, judgment in judgments.items():
                old_result = P.evaluate(judgment["pred"], raw, ids)
                new_result = P.evaluate(after_judgments[nid]["pred"], after_raw, after_ids)
                # Newly executable rules can turn unknown into a result. A condition
                # that already had a result must retain it, including false -> unknown.
                if old_result is not None and new_result is not old_result:
                    answer["problems"].append(f"{nid}: migration changes condition result from {old_result} to {new_result}; author the typed reading explicitly")
                elif old_result is None and new_result is True and not P.is_arrangement(judgment, raw):
                    # A newly executable calculation can expose a contradiction.
                    # Keep it visible in check without blocking its recording or
                    # refreshing the judgment's historical review snapshot.
                    answer["fired"].append(nid)
                    discovered.add(P._fired_failure(nid, after_judgments[nid]))
            answer["fired"].sort()
            answer["problems"] = [error for error in errors if error not in baseline and error not in discovered] + answer["problems"]
            after = shadow.read_bytes()
            answer["after_sha256"] = I._sha(after)
            if apply and not answer["problems"]:
                report = {'updates': [{'kind': 'add', 'id': nid, 'body': body} for nid, body in bodies.items()]}
                if I._report_private(doc, report):
                    raise ValueError('private or unclear source permission; migration left the record unchanged')
                if rec.read_bytes() != before:
                    raise ValueError("record changed during migration; retry")
                I._replace_record(rec, after)
                answer["applied"] = True
        return answer


def main(argv=None):
    parser = argparse.ArgumentParser(prog="kpop expressions", description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    convert = sub.add_parser("convert", help="return structured data without writing any record")
    convert.add_argument("text")
    convert.add_argument("--predicate", action="store_true")
    convert.add_argument('--readable', action='store_true', help='return the compact expr storage form')
    migrate_parser = sub.add_parser("migrate", help="preview unambiguous conversions; --apply writes a clean result")
    migrate_parser.add_argument("--record")
    migrate_parser.add_argument("--apply", action="store_true")
    migrate_parser.add_argument('--readable', action='store_true', help='convert supported active formulas to expr; preserve historical snapshots')
    for command in (convert, migrate_parser):
        command.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.action == "convert":
            expression = E.convert(args.text, args.predicate)
            if args.readable:
                expression = E.readable(expression, args.predicate)
            result = {"expression": expression, "display": E.text(expression), "references": E.refs(expression)}
        else:
            result = migrate(args.record, args.apply, args.readable)
        print(json.dumps(result, ensure_ascii=False))
        return 1 if args.action == "migrate" and args.apply and result["problems"] else 0
    except (ValueError, SyntaxError, OSError, RecursionError) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
