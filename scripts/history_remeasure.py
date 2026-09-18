"""Private history-backed prospective support for :mod:`remeasure`.

This module prepares existing history mutations and projects them in memory.  It
never publishes a mutation and never edits the knowledge record.
"""
import copy
import datetime
import glob
import os
import uuid

import provenance as P


def _peer(name):
    return P._peer(name)


def _entry(paths):
    first = (sorted(glob.glob(paths[0])) or [paths[0]])[0]
    return os.path.abspath(first)


def _refusal(error):
    code = getattr(error, "code", None)
    return (str(code) + ": " if code and str(code) not in str(error) else "") + str(error)


def prepare(paths, base, observed_at=None):
    """Prepare selected named layers and return their detached accepted candidate.

    Contributions are observations and remain in the candidate context.  A
    physical hypothesis has no equivalent admitted history operation, so this
    adapter refuses rather than dropping or treating it as accepted state.
    """
    operations = _peer("reasoning.operations")
    HH = _peer("history_hypotheses")
    prospective = _peer("history_prospective")
    observed_at = observed_at or datetime.datetime.now(datetime.timezone.utc)
    source = getattr(base, "_operation_source", None)
    captured = source.history_capture if source is not None else None
    if captured is None:
        raise P.Refused("incomplete - history remeasure requires the captured history authority")

    snapshot = operations.snapshot_for(base).to_data()
    selected, physical = [], []
    heads = []
    for name, hypothesis in sorted(base.hypotheses.items()):
        kind = hypothesis.get("kind")
        if kind == "contribution":
            continue
        if kind == HH.KIND:
            if hypothesis.get("error"):
                raise P.Refused("incomplete - named history hypothesis " + name + ": " + hypothesis["error"])
            selected.append(name)
            heads.append((name, copy.deepcopy(hypothesis.get("head") or {})))
        else:
            physical.append(name)
    if physical:
        raise P.Refused("incomplete - physical hypotheses have no faithful admitted history path: "
                        + ", ".join(physical))

    virtual = captured
    candidate = base
    if selected:
        try:
            mutation = HH.prepare_fold(
                _entry(paths), selected,
                because="prospective remeasure of selected named history",
                by="remeasure", operation="remeasure-fold-" + uuid.uuid4().hex,
                recorded_at=observed_at.isoformat(), capture=captured)
            candidate = operations.prepared(base, mutation)
            virtual = prospective._prospective_capture(captured, mutation)
        except (ValueError, SystemExit) as error:
            raise P.Refused("refused - history remeasure cannot admit the named fold: "
                            + _refusal(error)) from None
    return {"base": base, "candidate": candidate, "capture": virtual,
            "selected": selected, "heads": heads, "observed_at": observed_at,
            "source_snapshot": snapshot}


def measured(paths, prepared, differing, why):
    """Prepare and assess one final measured batch over a virtual history capture."""
    operations = _peer("reasoning.operations")
    authoring = _peer("history_authoring")
    prospective = _peer("history_prospective")
    HH = _peer("history_hypotheses")
    contract = _peer("reasoning.contract")
    observed_at = prepared["observed_at"]
    day = observed_at.date().isoformat()
    actions = [{"kind": "set", "id": identifier, "value": value,
                "as_of": day, "why": why(name)}
               for identifier, (name, _body, value) in sorted(differing.items())]
    try:
        mutation = authoring.prepare_batch(
            _entry(paths), actions, by="remeasure",
            operation="remeasure-values-" + uuid.uuid4().hex,
            recorded_at=observed_at.isoformat(), capture=prepared["capture"],
            context={"kind": "prospective_remeasure", "observation_date": day})
    except (ValueError, SystemExit) as error:
        raise P.Refused("refused - history remeasure cannot admit the measured values: "
                        + _refusal(error)) from None

    original = prepared["source_snapshot"]
    context = copy.deepcopy(original["context"])
    original_view = context.get("history_view")
    for key in ("history", "history_hypotheses", "history_view", "operation"):
        context.pop(key, None)
    context["operation_source"] = {
        "snapshot_id": operations.snapshot_for(prepared["base"]).snapshot_id,
        "context_digest": contract.digest(original["context"]),
        "history_view": original_view,
    }
    context["remeasure"] = {"version": 1, "phase": "prospective",
                            "observation_date": day,
                            "folded_hypotheses": list(prepared["selected"])}
    remaining = {name: value for name, value in original["hypotheses"].items()
                 if value.get("kind") != HH.KIND}
    snapshot = prospective.snapshot_after(
        prepared["capture"], mutation, context=context, hypotheses=remaining,
        as_of=original["as_of"])
    data = snapshot.to_data()
    document = P.Record(data["document"])
    document.hypotheses = copy.deepcopy(data["hypotheses"])
    document._operation_source = prepared["base"]._operation_source
    document = operations.bind(document, snapshot)

    # Set changes the measured value and observation day.  Existing source
    # locator and applicability fields are not a license for remeasure to edit.
    before = P.bodies(prepared["candidate"])
    after = P.bodies(document)
    for identifier in differing:
        for field in ("at", "applies"):
            if before[identifier].get(field) != after[identifier].get(field):
                raise P.Refused("refused - prospective remeasure changed " + identifier + "." + field)

    head_results = []
    for name, head in prepared["heads"]:
        expression = head.get("wrong_if")
        truth, result = operations.condition(document, expression)
        if expression is not None:
            head_results.append({"name": name, "truth": truth,
                                 "result": result or {"status": "unavailable"}})
    findings = operations.findings(operations.world(document).context)
    return {"document": document, "mutation": mutation, "findings": findings,
            "head_results": head_results}


def assessed(prepared):
    """Assess an unchanged prospective fold with the exact core evaluator."""
    operations = _peer("reasoning.operations")
    document = prepared["candidate"]
    heads = []
    for name, head in prepared["heads"]:
        expression = head.get("wrong_if")
        truth, result = operations.condition(document, expression)
        if expression is not None:
            heads.append({"name": name, "truth": truth,
                          "result": result or {"status": "unavailable"}})
    return {"document": document,
            "findings": operations.findings(operations.world(document).context),
            "head_results": heads}
