import Lean

open Lean

namespace EpistemicCore

inductive Truth where
  | yes | no | unknown
  deriving DecidableEq, Repr

def triOr : Truth → Truth → Truth
  | .yes, _ => .yes | _, .yes => .yes
  | .no, .no => .no | _, _ => .unknown

def truthJson : Truth → Json
  | .yes => .bool true | .no => .bool false | .unknown => .null

inductive Value where
  | number (value : JsonNumber)
  | text (value : String)
  | boolean (value : Bool)

def Value.toJson : Value → Json
  | .number n => .num n | .text s => .str s | .boolean b => .bool b

def valueOf : Json → Option Value
  | .num n => some (.number n)
  | .str s => some (.text s)
  | .bool b => some (.boolean b)
  | _ => none

def valueJson : Option Value → Json
  | some value => value.toJson | none => .null

def equalValue : Value → Value → Option Bool
  | .number a, .number b => some (compare a b == .eq)
  | .text a, .text b => some (a == b)
  | .boolean a, .boolean b => some (a == b)
  | _, _ => none

-- These wrappers intentionally have no coercion between them.
structure Current where
  value : Option Value
  role : String

structure AtReview where
  value : Option Value

def changed (current : Current) (reviewed : AtReview) : Truth :=
  match current.value, reviewed.value with
  | some a, some b => match equalValue a b with
      | some true => .no | some false => .yes | none => .unknown
  | _, _ => .unknown

inductive Op where
  | eq | ne | lt | le | gt | ge
  deriving DecidableEq

def applyOp (op : Op) (order : Ordering) : Bool :=
  match op with
  | .eq => order == .eq | .ne => order != .eq
  | .lt => order == .lt | .le => order != .gt
  | .gt => order == .gt | .ge => order != .lt

def compareCurrent (op : Op) (a b : Current) : Truth :=
  let result : Option Bool := match a.value, b.value with
    | some (.number x), some (.number y) => some (applyOp op (compare x y))
    | some x, some y =>
        if op == .eq then equalValue x y
        else if op == .ne then (equalValue x y).map (! ·)
        else none
    | _, _ => none
  match result with
  | some true => .yes | some false => .no | none => .unknown

structure Assessment where
  movement : Truth
  falsifier : Truth

def Assessment.acceptsFalsified (state : Assessment) (expected : Bool) : Bool :=
  match state.falsifier with
  | .unknown => false | .yes => expected | .no => !expected

def Assessment.reviewTrigger (state : Assessment) : Truth := triOr state.movement state.falsifier

theorem missing_current_is_unknown (op : Op) (b : Current) :
    compareCurrent op { value := none, role := "unavailable" } b = .unknown := by
  cases b with
  | mk value role => cases value <;> rfl

theorem changed_does_not_falsify (movement : Truth) :
    Assessment.acceptsFalsified ⟨movement, .no⟩ true = false := rfl

theorem unknown_is_not_a_negative_result (movement : Truth) :
    Assessment.acceptsFalsified ⟨movement, .unknown⟩ false = false := rfl

theorem unchanged_and_untriggered : triOr .no .no = .no := rfl

def field (object : Json) (key : String) : Json :=
  (object.getObjVal? key).toOption.getD .null

def stringField (object : Json) (key : String) : String :=
  ((field object key).getStr?).toOption.getD ""

def nodeOf (record : Json) (id : String) : Json := field (field record "nodes") id
def bodyOf (record : Json) (id : String) : Json :=
  let node := nodeOf record id
  if field node "assessment_body" != .null then field node "assessment_body" else field node "body"

def hasPriorRole (node : Json) : Bool :=
  (((field node "states").getArr?).toOption.getD #[]).any (· == Json.str "prior")

def hasState (node : Json) (state : String) : Bool :=
  (((field node "states").getArr?).toOption.getD #[]).any (· == Json.str state)

def refersToRecord (record : Json) (text : String) : Bool :=
  match field record "nodes" with
  | .obj entries => entries.foldl (init := false) fun found id _ =>
      found || (id.contains '.' && (text.splitOn id).length > 1)
  | _ => false

def ambiguousValue (record : Json) : Json → Bool
  | .str text => refersToRecord record text
  | _ => false

def currentOf (record : Json) (id : String) : Current := Id.run do
  let node := nodeOf record id
  let body := bodyOf record id
  if field body "rule" != .null then
    return { value := none, role := "unevaluated_rule" }
  -- The native reader also stores expressions in v/quoted. Referential text needs an
  -- explicit literal/derived distinction; conservatively refuse to compare it here.
  if let some value := valueOf (field body "v") then
    if ambiguousValue record (field body "v") then
      return { value := none, role := "unevaluated_referential_text" }
    return { value, role := if hasPriorRole node then "prior_premise" else "recorded_value" }
  if let some value := valueOf (field body "quoted") then
    if ambiguousValue record (field body "quoted") then
      return { value := none, role := "unevaluated_referential_text" }
    return { value, role := "recorded_quote" }
  if let some value := valueOf (field body "verdict") then
    return { value, role := "recorded_judgment_claim" }
  if let some stamp := ((field body "read").getStr?).toOption then
    return { value := some (.text ("read " ++ stamp)), role := "source_read_stamp" }
  return { value := none, role := "unavailable" }

def reviewedOf (judgment : Json) (id : String) : AtReview :=
  { value := valueOf (field (field judgment "seen") id) }

def parseOp : String → Option Op
  | "==" => some .eq | "!=" => some .ne | "<" => some .lt
  | "<=" => some .le | ">" => some .gt | ">=" => some .ge | _ => none

structure PredicateResult where
  evaluation : Truth
  reason : String
  reads : List String

def predicateSpace (c : Char) : Bool :=
  c == ' ' || c == '\t' || c == '\r' || c == '\n'

def barePredicateReference (text : String) : Bool :=
  !text.isEmpty && text.toList.all (fun c =>
    c.toNat >= 0x20 && !predicateSpace c &&
    !(['\'', '"', '\\', '[', ']', '{', '}', '<', '>', '=', '!'] : List Char).contains c)

def predicateParts (text : String) : Option (String × String × String) := do
  -- Split only the reference and operator. Keep the complete RHS, including
  -- significant spaces inside quotes; ASCII whitespace outside it is ignored.
  let input := text.toList.dropWhile predicateSpace
  let left := input.takeWhile (fun c => !predicateSpace c)
  let afterLeft := (input.dropWhile (fun c => !predicateSpace c)).dropWhile predicateSpace
  let operator := afterLeft.takeWhile (fun c => !predicateSpace c)
  let right := ((afterLeft.dropWhile (fun c => !predicateSpace c)).dropWhile predicateSpace).reverse
  let right := (right.dropWhile predicateSpace).reverse
  if left.isEmpty || operator.isEmpty || right.isEmpty then none
  else some (String.ofList left, String.ofList operator, String.ofList right)

def singleQuotedChars : List Char → List Char → Option String
  | [], _ => none
  | c :: rest, reversed =>
      if c == '\'' then
        if rest.isEmpty then some (String.ofList reversed.reverse) else none
      else if c == '\\' then
        match rest with
        | escaped :: tail =>
            if escaped == '\\' || escaped == '\'' then singleQuotedChars tail (escaped :: reversed)
            else none
        | [] => none
      else if c.toNat < 0x20 then none
      else singleQuotedChars rest (c :: reversed)

inductive PredicateOperand where
  | literal (value : Value)
  | reference (id : String)

def predicateOperand (text : String) : Except String PredicateOperand :=
  -- JSON scalar literals keep JSON's escaping rules. Single quotes are a small
  -- additional syntax: only \\ and \' escapes, no raw U+0000..U+001F or suffix.
  -- The native reader merely strips quote characters; this checked subset does
  -- not inherit its permissive handling of malformed quotes or bare prose.
  match Json.parse text with
  | .ok literal => match valueOf literal with
      | some value => .ok (.literal value)
      | none => .error "unsupported_literal"
  | .error _ =>
      match text.toList with
      | '\'' :: rest => match singleQuotedChars rest [] with
          | some value => .ok (.literal (.text value))
          | none => .error "malformed_literal"
      | _ =>
          if barePredicateReference text then .ok (.reference text)
          else .error "unsupported_or_malformed_rhs"

def evaluatePredicate (record : Json) (dependencies : List String) (text : String) : PredicateResult := Id.run do
  -- One whitespace-separated reference, operator, and literal/reference RHS.
  -- No expression evaluation: richer or malformed syntax remains unknown.
  let some (left, operator, right) := predicateParts text
    | return ⟨.unknown, if text.isEmpty then "not_declared" else "unsupported_syntax", []⟩
  if !barePredicateReference left then
    return ⟨.unknown, "unsupported_syntax", []⟩
  let some op := parseOp operator
    | return ⟨.unknown, "unsupported_operator", []⟩
  if !dependencies.contains left then
    return ⟨.unknown, "undeclared_reference", [left]⟩
  match predicateOperand right with
  | .error reason => return ⟨.unknown, reason, [left]⟩
  | .ok (.literal value) =>
      let answer := compareCurrent op (currentOf record left) { value, role := "literal" }
      return ⟨answer, if answer == .unknown then "missing_or_incompatible_value" else "evaluated_current_values", [left]⟩
  | .ok (.reference id) =>
      if !dependencies.contains id then
        return ⟨.unknown, "undeclared_or_unsupported_rhs", [left, id]⟩
      let answer := compareCurrent op (currentOf record left) (currentOf record id)
      return ⟨answer, if answer == .unknown then "missing_or_incompatible_value" else "evaluated_current_values", [left, id]⟩

def premiseJson (record judgment : Json) (id : String) : Json :=
  let current := currentOf record id
  let reviewed := reviewedOf judgment id
  Json.mkObj [
    ("id", toJson id), ("role", toJson current.role),
    ("current_recorded_value", valueJson current.value),
    ("value_at_review", valueJson reviewed.value),
    ("changed", truthJson (changed current reviewed)),
    ("confidence_owner", if current.role == "prior_premise" then toJson id else .null),
    ("source_ref", toJson ("node:" ++ id))]

def judgmentConfidence (judgment : Json) : Option Value := valueOf (field judgment "confidence")

def dependenciesOf (body : Json) : Except String (List String) := do
  let values ← (field body "rests_on").getArr?
  values.toList.mapM Json.getStr?

def movementOf (record body : Json) (dependencies : List String) : Truth :=
  dependencies.foldl
    (fun prior dep => triOr prior (changed (currentOf record dep) (reviewedOf body dep))) .no

def predicateJson (text : String) (result : PredicateResult) : Json :=
  Json.mkObj [("expression", toJson text), ("holds_on_current_values", truthJson result.evaluation),
    ("reason", toJson result.reason), ("reads", toJson result.reads)]

def bundle (record : Json) (id : String) : Except String Json := do
  let node := nodeOf record id
  if stringField node "kind" != "judgment" then throw "not_a_judgment"
  let body := bodyOf record id
  let claim := stringField body "verdict"
  if claim.isEmpty then throw "missing_claim_body; an ID is not a claim"
  let dependencies ← dependenciesOf body
  match field body "wrong_if" with
  | .null | .str _ => pure ()
  | _ => throw "unsupported_predicate_type; wrong_if must be text"
  let predicate := stringField body "wrong_if"
  let result := evaluatePredicate record dependencies predicate
  let movement := movementOf record body dependencies
  let assessment : Assessment := ⟨movement, result.evaluation⟩
  return Json.mkObj [
    ("id", toJson id), ("recorded_claim", toJson claim),
    ("recorded_status", field (field node "body") "status"),
    ("declared_conflict", toJson (hasState node "contested")),
    ("conflict_scope", toJson "Declared record flag only; it does not establish or refute the claim."),
    ("premises", toJson (dependencies.map (premiseJson record body))),
    ("premise_changed", truthJson movement),
    ("falsifier", predicateJson predicate result),
    ("mechanical_review_trigger", truthJson assessment.reviewTrigger),
    ("human_reopener", Json.mkObj [("declaration", field body "reopened_by"), ("evaluation", toJson "not_evaluated")]),
    ("declared_gap", Json.mkObj [("declaration", field body "blocked_on"), ("evaluation", toJson "not_evaluated")]),
    ("declared_judgment_confidence", valueJson (judgmentConfidence body)),
    ("confidence_note", toJson "Premise confidence is not inherited by the judgment."),
    ("scope", toJson "Computed from this supplied record. Not a proof of source truth or authorization.")]

def valueMatches (actual : Option Value) (expected : Json) : Bool :=
  match actual, valueOf expected with
  | some a, some b => (equalValue a b).getD false
  | _, _ => false

def checkAssertion (record assertion : Json) : Json := Id.run do
  let kind := stringField assertion "kind"
  let id := stringField assertion "id"
  let expected := field assertion "expected"
  let allowed := if kind == "at_review" then ["kind", "id", "expected", "dependency"]
                 else ["kind", "id", "expected"]
  let validFields := match assertion with
    | .obj fields => fields.all (fun key _ => allowed.contains key)
    | _ => false
  if !validFields then
    return Json.mkObj [("kind", toJson kind), ("id", toJson id), ("accepted", .bool false),
      ("reason", toJson "unsupported_assertion_fields; extra prose is not certified")]
  let body := bodyOf record id
  let mut actual : Json := .null
  let mut accepted := false
  let mut reason := "unsupported_or_unavailable_claim"
  if kind == "current" then
    let value := (currentOf record id).value
    actual := valueJson value
    accepted := valueMatches value expected
    reason := "checked_against_current_recorded_value"
  else if kind == "at_review" then
    let dependency := stringField assertion "dependency"
    let value := (reviewedOf body dependency).value
    actual := valueJson value
    accepted := valueMatches value expected
    reason := "checked_against_historical_review_snapshot"
  else if kind == "falsifier_holds" then
    if let .ok dependencies := dependenciesOf body then
      let result := evaluatePredicate record dependencies (stringField body "wrong_if")
      actual := truthJson result.evaluation
      let assessment : Assessment := ⟨movementOf record body dependencies, result.evaluation⟩
      if let .bool value := expected then
        accepted := stringField (nodeOf record id) "kind" == "judgment" && assessment.acceptsFalsified value
      reason := result.reason
  else if kind == "judgment_confidence" then
    let value := judgmentConfidence body
    actual := valueJson value
    accepted := stringField (nodeOf record id) "kind" == "judgment" && valueMatches value expected
    reason := "only_explicit_judgment_confidence; no_premise_inheritance"
  else if kind == "claim_text" then
    let value := valueOf (field body "verdict")
    actual := valueJson value
    accepted := stringField (nodeOf record id) "kind" == "judgment" && valueMatches value expected
    reason := "exact_recorded_claim_only; an_ID_does_not_supply_claim_text"
  return Json.mkObj [("kind", toJson kind), ("id", toJson id), ("accepted", toJson accepted),
    ("expected", expected), ("actual", actual), ("reason", toJson reason)]

structure ScanRow where
  id : String
  result : Except String Json

def ScanRow.succeeded (row : ScanRow) : Bool :=
  match row.result with | .ok _ => true | .error _ => false

def successCount (rows : List ScanRow) : Nat := (rows.filter ScanRow.succeeded).length
def errorCount (rows : List ScanRow) : Nat := (rows.filter (fun row => !row.succeeded)).length

theorem scan_accounts_for_every_row (rows : List ScanRow) :
    successCount rows + errorCount rows = rows.length := by
  induction rows with
  | nil => rfl
  | cons row rest ih =>
      cases h : row.succeeded <;>
        simp [successCount, errorCount, h] at ih ⊢ <;> omega

def declarationView (record : Json) (row : ScanRow) : Json :=
  let body := bodyOf record row.id
  let executable := match row.result with
    | .ok value => field value "falsifier"
    | .error message => Json.mkObj [
        ("expression", field body "wrong_if"), ("holds_on_current_values", .null),
        ("reads", toJson ([] : List String)), ("reason", toJson "assessment_error"), ("error", toJson message)]
  Json.mkObj [("executable", executable),
    ("blocked_on", Json.mkObj [("declaration", field body "blocked_on"), ("evaluation", toJson "not_evaluated")]),
    ("reopened_by", Json.mkObj [("declaration", field body "reopened_by"), ("evaluation", toJson "not_evaluated")])]

def eventConditions (conditions : Json) (ids : List String) : Json :=
  Json.mkObj ((ids.filter (fun id => field conditions id != .null)).map fun id => (id, field conditions id))

def changeIdentity (premise : Json) : Json :=
  Json.mkObj ((["id", "role", "current_recorded_value", "value_at_review"] : List String).map
    fun name => (name, field premise name))

def eventsOf (record conditions : Json) (assessments errors : List Json) : Json := Id.run do
  let mut events : List Json := []
  let nodes := ((field record "nodes").getObj?).toOption.getD {}
  let conflicts := nodes.foldl (init := []) fun acc id node =>
    if hasState node "contested" then id :: acc else acc
  for id in conflicts do
    events := events ++ [Json.mkObj [("kind", toJson "contested"), ("id", toJson id),
      ("affected", toJson [id]), ("details_ref", toJson ("node:" ++ id)),
      ("basis", toJson "declared state; conflict content may need native hypotheses"),
      ("conditions", eventConditions conditions [id])]]
  for error in errors do
    let id := stringField error "id"
    events := events ++ [Json.mkObj [("kind", toJson "unreadable"), ("id", toJson id),
      ("affected", toJson [id]), ("error", field error "error"),
      ("conditions", eventConditions conditions [id])]]
  let changes := assessments.flatMap fun assessment =>
    let premises := (((field assessment "premises").getArr?).toOption.getD #[]).toList.zipIdx
    (premises.filter fun pair => field pair.1 "changed" == .bool true).map fun pair =>
      (changeIdentity pair.1, stringField assessment "id", pair.2)
  let identities := (changes.map (fun row => row.1)).eraseDups
  for identity in identities do
    let matching := changes.filter (fun row => row.1 == identity)
    let affected := (matching.map (fun row => row.2.1)).eraseDups.mergeSort (· ≤ ·)
    let fields := (["id", "role", "current_recorded_value", "value_at_review"] : List String).map
      fun name => (name, field identity name)
    let refs := Json.mkObj (matching.map fun row =>
      (row.2.1, toJson ("checked:" ++ row.2.1 ++ "#/premises/" ++ toString row.2.2)))
    events := events ++ [Json.mkObj (fields ++ [("kind", toJson "premise_change"),
      ("affected", toJson affected), ("premise_refs", refs), ("conditions", eventConditions conditions affected)])]
  for assessment in assessments do
    let id := stringField assessment "id"
    let predicate := field assessment "falsifier"
    let missing := (((field assessment "premises").getArr?).toOption.getD #[]).toList.filter
      (fun p => field p "changed" == .null)
    let triggered := field predicate "holds_on_current_values" == .bool true
    let condition := field conditions id
    let undeclared := stringField predicate "reason" == "not_declared" &&
      field (field condition "blocked_on") "declaration" == .null &&
      field (field condition "reopened_by") "declaration" == .null
    if triggered || !missing.isEmpty || (field predicate "holds_on_current_values" == .null &&
        stringField predicate "reason" != "not_declared") || undeclared then
      events := events ++ [Json.mkObj [("kind", toJson (if triggered then "triggered" else "uncheckable")),
        ("id", toJson id), ("affected", toJson [id]), ("falsifier", predicate),
        ("unknown_comparisons", toJson (missing.map (fun p => stringField p "id"))),
        ("comparison_details", toJson missing),
        ("details_ref", toJson ("checked:" ++ id)), ("conditions", eventConditions conditions [id])]]
  let rank (event : Json) : Nat := match stringField event "kind" with
    | "contested" | "unreadable" => 0 | "triggered" => 1 | "uncheckable" => 2 | _ => 3
  let size (event : Json) := (((field event "affected").getArr?).toOption.getD #[]).size
  let sorted := events.mergeSort fun a b =>
    if rank a != rank b then rank a < rank b
    else if size a != size b then size a > size b
    else a.compress ≤ b.compress
  return Json.mkObj (sorted.zipIdx.map fun (event, index) => ("e" ++ toString (index + 1), event))

def scanRecord (record : Json) : Except String Json := do
  let nodes ← (field record "nodes").getObj?
  let ids := nodes.foldl (init := []) fun ids id node =>
    if stringField node "kind" == "judgment" then id :: ids else ids
  let rows := ids.map (fun id => ScanRow.mk id (bundle record id))
  let assessments := rows.filterMap fun row => match row.result with
    | .ok value => some value | .error _ => none
  let errors := rows.filterMap fun row => match row.result with
    | .ok _ => none
    | .error message => some (Json.mkObj [("id", toJson row.id), ("error", toJson message)])
  let count (predicate : Json → Bool) := (assessments.filter predicate).length
  let conditions := Json.mkObj (rows.map fun row => (row.id, declarationView record row))
  return Json.mkObj [
    ("assessments", toJson assessments), ("errors", toJson errors),
    ("conditions", conditions), ("events", eventsOf record conditions assessments errors),
    ("counts", Json.mkObj [
      ("nodes", toJson (nodes.foldl (init := 0) fun n _ _ => n + 1)),
      ("judgments", toJson rows.length), ("assessed", toJson (successCount rows)),
      ("errors", toJson (errorCount rows)),
      ("contested", toJson (nodes.foldl (init := 0) fun n _ node =>
        n + if hasState node "contested" then 1 else 0)),
      ("human_reopener_declared", toJson ((ids.filter fun id => field (bodyOf record id) "reopened_by" != .null).length)),
      ("gap_declared", toJson ((ids.filter fun id => field (bodyOf record id) "blocked_on" != .null).length)),
      ("premise_changed", toJson (count (fun value => field value "premise_changed" == .bool true))),
      ("premise_comparison_incomplete", toJson (count (fun value =>
        (((field value "premises").getArr?).toOption.getD #[]).any
          (fun premise => field premise "changed" == .null)))),
      ("falsifier_triggered", toJson (count (fun value => field (field value "falsifier") "holds_on_current_values" == .bool true))),
      ("falsifier_not_triggered", toJson (count (fun value => field (field value "falsifier") "holds_on_current_values" == .bool false))),
      ("falsifier_not_declared", toJson (count (fun value => stringField (field value "falsifier") "reason" == "not_declared"))),
      ("falsifier_unknown", toJson (count (fun value =>
        field (field value "falsifier") "holds_on_current_values" == .null &&
        stringField (field value "falsifier") "reason" != "not_declared")))])]

structure ViewCell where
  kind : String
  key : String
  topic : String
  members : List String
  conflicts : List String
  questions : List String
  attention : List String
  edgeIndices : List Nat
  linkRef : String
  relationCounts : Json

def coversExactly (expected actual : List String) : Bool := decide (expected.Perm actual)

def keepsConflicts (conflicts : List String) (cells : List ViewCell) : Bool :=
  conflicts.all fun id => cells.any fun cell => cell.members.contains id && cell.conflicts.contains id

def keepsLinks (edgeCount : Nat) (cells : List ViewCell) : Bool :=
  decide ((List.range edgeCount).Perm (cells.flatMap ViewCell.edgeIndices)) &&
  cells.all fun cell => cell.edgeIndices.isEmpty || !cell.linkRef.isEmpty

def acceptsView (ids conflicts : List String) (edgeCount : Nat) (cells : List ViewCell) : Bool :=
  coversExactly ids (cells.flatMap ViewCell.members) &&
  keepsConflicts conflicts cells && keepsLinks edgeCount cells

theorem accepted_view_preserves_conflict_signal
    (ids conflicts : List String) (edgeCount : Nat) (cells : List ViewCell)
    (h : acceptsView ids conflicts edgeCount cells = true) :
    keepsConflicts conflicts cells = true := by
  simp only [acceptsView, Bool.and_eq_true] at h
  exact h.1.2

theorem accepted_view_accounts_for_every_link
    (ids conflicts : List String) (edgeCount : Nat) (cells : List ViewCell)
    (h : acceptsView ids conflicts edgeCount cells = true) :
    (List.range edgeCount).Perm (cells.flatMap ViewCell.edgeIndices) := by
  simp only [acceptsView, Bool.and_eq_true] at h
  have links := h.2
  simp only [keepsLinks, Bool.and_eq_true, decide_eq_true_eq] at links
  exact links.1

def stringList (value : Json) : Except String (List String) := do
  (← value.getArr?).toList.mapM Json.getStr?

def parseCell (value : Json) : Except String ViewCell := do
  return { kind := ← (field value "kind").getStr?, key := ← (field value "key").getStr?,
           topic := ← (field value "topic").getStr?,
           members := ← stringList (field value "members"),
           conflicts := ← stringList (field value "conflicts"),
           questions := ← stringList (field value "questions"),
           attention := ← stringList (field value "attention"),
           edgeIndices := ← (← (field value "edge_indices").getArr?).toList.mapM Json.getNat?,
           linkRef := ← (field value "links_ref").getStr?, relationCounts := field value "relation_counts" }

def cellIdentityCorrect (record : Json) (ids : List String) (cell : ViewCell) : Bool :=
  if cell.kind == "node" then ids.contains cell.key && coversExactly [cell.key] cell.members &&
    cell.topic == stringField (field record "navigation_leaf_routes") cell.key
  else if cell.kind == "group" then
    match stringList (field (field record "navigation_routes") cell.key) with
    | .ok expected => coversExactly expected cell.members && cell.topic == cell.key
    | .error _ => false
  else false

def checkLinkMap (record : Json) (ids : List String) (edges : Array Json) (view : Json) : Except String Bool := do
  if field view "link_map" == .null then return field view "link_cells" == .null
  let rows ← (field view "link_map").getArr?
  let cells ← (← (field view "link_cells").getArr?).toList.mapM parseCell
  let allMembers := cells.flatMap ViewCell.members
  let allIndices ← rows.toList.flatMapM fun row => do
    (← (field row "edge_indices").getArr?).toList.mapM Json.getNat?
  let memberOf (key id : String) : Bool :=
    cells.any fun cell => cell.key == key && cell.members.contains id
  let owned := rows.all fun row =>
    !(((field row "edge_indices").getArr?).toOption.getD #[]).isEmpty &&
    (((field row "edge_indices").getArr?).toOption.getD #[]).all fun value =>
      let index := value.getNat?.toOption.getD edges.size
      let edge := edges[index]?.getD .null
      let target := stringField edge "to"
      index < edges.size && stringField row "rel" == stringField edge "rel" &&
      memberOf (stringField row "from") (stringField edge "from") &&
      (memberOf (stringField row "to") target ||
       (!ids.contains target && stringField row "to" == "unresolved:" ++ target))
  return coversExactly ids allMembers && cells.all (cellIdentityCorrect record ids) &&
    decide ((List.range edges.size).Perm allIndices) && owned

def guardView (record view : Json) : Except String Json := do
  let nodes ← (field record "nodes").getObj?
  let ids := nodes.foldl (init := []) fun acc id _ => id :: acc
  let conflicts := nodes.foldl (init := []) fun acc id node =>
    if hasState node "contested" then id :: acc else acc
  let edges ← (field record "edges").getArr?
  let cells ← (← (field view "cells").getArr?).toList.mapM parseCell
  let scan ← scanRecord record
  let questions := ids.filter fun id => hasState (nodeOf record id) "question"
  let eventEntries ← (field scan "events").getObj?
  let review := eventEntries.foldl (init := []) fun acc _ event =>
    acc ++ (((field event "affected").getArr?).toOption.getD #[]).toList.filterMap
      (fun value => value.getStr?.toOption)
  let localCorrect := cells.all fun cell =>
    let relations := (cell.edgeIndices.map fun i => stringField (edges[i]?.getD .null) "rel").eraseDups
    let expectedRelations := Json.mkObj (relations.map fun rel =>
      (rel, toJson ((cell.edgeIndices.filter fun i => stringField (edges[i]?.getD .null) "rel" == rel).length)))
    cellIdentityCorrect record ids cell && cell.linkRef == "links:" ++ cell.key &&
    coversExactly (cell.members.filter conflicts.contains) cell.conflicts &&
    coversExactly (cell.members.filter questions.contains) cell.questions &&
    coversExactly (cell.members.filter review.contains) cell.attention &&
    cell.relationCounts == expectedRelations &&
    cell.edgeIndices.all fun index =>
      index < edges.size && cell.members.contains (stringField (edges[index]?.getD .null) "from")
  let countsCorrect := field view "counts" == field scan "counts"
  let declarationsAddressed := stringField view "conditions_ref" == "conditions:/"
  let eventsCorrect := field view "events" == field scan "events"
  let rawLinksCorrect := field view "links" == field record "edges"
  let linkMapCorrect ← checkLinkMap record ids edges view
  let accepted := acceptsView ids conflicts edges.size cells && localCorrect && countsCorrect && declarationsAddressed && linkMapCorrect && eventsCorrect && rawLinksCorrect
  return Json.mkObj [("accepted", toJson accepted),
    ("coverage", toJson (coversExactly ids (cells.flatMap ViewCell.members))),
    ("conflicts", toJson (keepsConflicts conflicts cells)),
    ("links", toJson (keepsLinks edges.size cells)), ("cell_ownership", toJson localCorrect),
    ("counts", toJson countsCorrect), ("conditions_addressed", toJson declarationsAddressed),
    ("link_map", toJson linkMapCorrect), ("events", toJson eventsCorrect), ("raw_links", toJson rawLinksCorrect)]

def handle (request : Json) : Except String Json := do
  let record := field request "record"
  if stringField request "operation" == "scan" then return ← scanRecord record
  if stringField request "operation" == "guard_view" then return ← guardView record (field request "view")
  let id := stringField request "id"
  let card ← bundle record id
  let assertions ← match field request "assertions" with
    | .null => pure #[] | .arr values => pure values | _ => throw "assertions_must_be_an_array"
  let checks := assertions.map (checkAssertion record)
  return Json.mkObj [("bundle", card), ("assertion_checks", .arr checks),
    ("assertions_requested", toJson assertions.size),
    ("assertions_accepted", if assertions.isEmpty then .null
      else toJson (checks.all (fun check => field check "accepted" == .bool true)))]

end EpistemicCore

def main : IO UInt32 := do
  let input ← (← IO.getStdin).readToEnd
  let result := (Json.parse input).bind EpistemicCore.handle
  match result with
  | .error message =>
      (← IO.getStdout).putStrLn (Json.mkObj [("error", toJson message)]).compress
      return 1
  | .ok response =>
      (← IO.getStdout).putStrLn response.compress
      return if EpistemicCore.field response "assertions_accepted" == .bool false then 2 else 0
