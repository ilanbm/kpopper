import Protocol
import Kpopper.QueryProtocol
import Lean.Data.Json.Parser

/-! Canonical JSON KP4/KR4 adapter. It only translates the closed wire grammar
to and from `QueryProtocol`; all query meaning remains in the pure kernel. -/
namespace Kpopper.Query.Wire

open Lean
open Kpopper.Query

abbrev DecodeM := Except String

private def malformed : DecodeM α := .error "malformed_wire"

private def exactObject (value : Json) (keys : List String) : DecodeM (Std.TreeMap.Raw String Json compare) := do
  let .obj fields := value | malformed
  if fields.size != keys.length || !keys.all (fun key => (fields.get? key).isSome) then malformed
  return fields

private def field (value : Json) (name : String) : DecodeM Json :=
  value.getObjVal? name |>.mapError fun _ => "malformed_wire"

private def text (value : Json) : DecodeM String :=
  value.getStr? |>.mapError fun _ => "malformed_wire"

private def natural (value : Json) : DecodeM Nat :=
  value.getNat? |>.mapError fun _ => "malformed_wire"

private def boolean (value : Json) : DecodeM Bool :=
  value.getBool? |>.mapError fun _ => "malformed_wire"

private def array (value : Json) : DecodeM (Array Json) :=
  value.getArr? |>.mapError fun _ => "malformed_wire"

private def textArray (value : Json) : DecodeM (List String) := do
  let values ← array value
  values.toList.mapM text

private def decodeRational (value : Json) : DecodeM Rat := do
  let _ ← exactObject value ["denominator", "numerator", "type"]
  if (← text (← field value "type")) != "number" then malformed
  let numeratorText ← text (← field value "numerator")
  let denominatorText ← text (← field value "denominator")
  if numeratorText.length > 4097 || denominatorText.length > 4096 then malformed
  let some numerator := numeratorText.toInt? | malformed
  let some denominator := denominatorText.toNat? | malformed
  if denominator == 0 || toString numerator != numeratorText || toString denominator != denominatorText then
    malformed
  let result := mkRat numerator denominator
  if toString result.num != numeratorText || toString result.den != denominatorText then malformed
  return result

partial def decodeValue (value : Json) (depth : Nat := 0) : DecodeM Kpopper.Query.Value := do
  if depth > 128 then malformed
  let kind ← text (← field value "type")
  match kind with
  | "number" => return .scalar (.number (← decodeRational value))
  | "boolean" =>
      let _ ← exactObject value ["type", "value"]
      return .scalar (.boolean (← boolean (← field value "value")))
  | "text" =>
      let _ ← exactObject value ["type", "value"]
      return .scalar (.text (← text (← field value "value")))
  | "null" =>
      let _ ← exactObject value ["type"]
      return .scalar .null
  | "list" =>
      let _ ← exactObject value ["items", "type"]
      let values ← array (← field value "items")
      return makeArray (← values.toList.mapM fun item => decodeValue item (depth + 1))
  | "record" =>
      let _ ← exactObject value ["fields", "type"]
      let .obj fields := (← field value "fields") | malformed
      let decoded ← fields.foldlM (init := []) fun result key item => do
        return (key, ← decodeValue item (depth + 1)) :: result
      return makeObject decoded.reverse
  | _ => malformed

partial def decodeExpr (value : Json) (depth : Nat := 0) : DecodeM Expr := do
  if depth > 128 then malformed
  let .obj fields := value | malformed
  let keys := fields.foldl (init := []) fun result key _ => key :: result
  match keys.reverse with
  | ["column"] => return .column (← text (← field value "column"))
  | ["num"] =>
      let lexeme ← text (← field value "num")
      return .literal (.number (← Kpopper.parseNumber lexeme |>.mapError fun _ => "malformed_wire"))
  | ["bool"] => return .literal (.boolean (← boolean (← field value "bool")))
  | ["text"] => return .literal (.text (← text (← field value "text")))
  | ["null"] =>
      if !(← boolean (← field value "null")) then malformed
      return .literal .null
  | ["args", "op"] =>
      let name ← text (← field value "op")
      let args ← array (← field value "args")
      if name == "not" then
        if args.size != 1 then malformed
        return .negate (← decodeExpr args[0]! (depth + 1))
      if name == "and" || name == "or" then
        if args.size != 2 then malformed
        return .logic (name == "and") (← decodeExpr args[0]! (depth + 1))
          (← decodeExpr args[1]! (depth + 1))
      if args.size != 2 then malformed
      let op ← Kpopper.parseOp name |>.mapError fun _ => "malformed_wire"
      return .binary op (← decodeExpr args[0]! (depth + 1))
        (← decodeExpr args[1]! (depth + 1))
  | ["else", "if", "then"] =>
      return .conditional (← decodeExpr (← field value "if") (depth + 1))
        (← decodeExpr (← field value "then") (depth + 1))
        (← decodeExpr (← field value "else") (depth + 1))
  | ["list"] =>
      let values ← array (← field value "list")
      return .array (← values.toList.mapM fun item => decodeExpr item (depth + 1))
  | ["record"] =>
      let .obj entries := (← field value "record") | malformed
      let decoded ← entries.foldlM (init := []) fun result key item => do
        return (key, ← decodeExpr item (depth + 1)) :: result
      return .object decoded.reverse
  | ["field", "key"] =>
      return .field (← decodeExpr (← field value "field") (depth + 1))
        (← text (← field value "key"))
  | _ => malformed

private def decodeOperation (value : Json) : DecodeM Operation := do
  let .obj fields := value | malformed
  let version ← natural (← field value "version")
  if version != 1 then malformed
  let _scope ← text (← field value "scope")
  let name ← text (← field value "op")
  let keys := fields.foldl (init := []) fun result key _ => key :: result
  let keys := keys.reverse
  match name with
  | "filter" =>
      if keys != ["op", "scope", "version", "where"] then malformed
      return .filter (← decodeExpr (← field value "where"))
  | "project" =>
      if keys != ["op", "scope", "value", "version"] then malformed
      return .project (← decodeExpr (← field value "value"))
  | "select" =>
      if keys != ["op", "scope", "value", "version", "where"] then malformed
      return .select (← decodeExpr (← field value "where")) (← decodeExpr (← field value "value"))
  | "count" =>
      if keys != ["op", "scope", "version", "where"] then malformed
      return .count (← decodeExpr (← field value "where"))
  | "sum" =>
      if keys == ["op", "scope", "value", "version"] then
        return .sum none (← decodeExpr (← field value "value"))
      if keys == ["op", "scope", "value", "version", "where"] then
        return .sum (some (← decodeExpr (← field value "where"))) (← decodeExpr (← field value "value"))
      malformed
  | "all" =>
      if keys != ["op", "scope", "version", "where"] then malformed
      return .all (← decodeExpr (← field value "where"))
  | "any" =>
      if keys != ["op", "scope", "version", "where"] then malformed
      return .any (← decodeExpr (← field value "where"))
  | _ => malformed

private def decodeResources (value : Json) : DecodeM Resources := do
  let _ ← exactObject value ["candidates", "depth", "digits", "field_reads", "steps",
    "value_bytes", "value_depth", "value_nodes", "version"]
  return {
    version := ← text (← field value "version")
    steps := ← natural (← field value "steps")
    depth := ← natural (← field value "depth")
    digits := ← natural (← field value "digits")
    valueNodes := ← natural (← field value "value_nodes")
    valueDepth := ← natural (← field value "value_depth")
    valueBytes := ← natural (← field value "value_bytes")
    candidates := ← natural (← field value "candidates")
    fieldReads := ← natural (← field value "field_reads") }

private def decodeScopeWitness (value : Json) : DecodeM ScopeWitness := do
  let _ ← exactObject value ["definition_digest", "kind", "membership_digest",
    "projected_inputs_digest", "scope_id"]
  if (← text (← field value "kind")) != "scope" then malformed
  return { scopeId := ← text (← field value "scope_id")
           definitionDigest := ← text (← field value "definition_digest")
           membershipDigest := ← text (← field value "membership_digest")
           projectedInputsDigest := ← text (← field value "projected_inputs_digest") }

private def decodeNodeWitness (value : Json) : DecodeM NodeWitness := do
  let _ ← exactObject value ["fingerprint", "id", "kind"]
  if (← text (← field value "kind")) != "node" then malformed
  return { id := ← text (← field value "id"), fingerprint := ← text (← field value "fingerprint") }

private def decodeCell (value : Json) : DecodeM FieldStatus := do
  let status ← text (← field value "status")
  match status with
  | "known" =>
      let _ ← exactObject value ["status", "value"]
      return .known (← decodeValue (← field value "value"))
  | "missing" =>
      let _ ← exactObject value ["status"]
      return .missing
  | "contested" =>
      let _ ← exactObject value ["status"]
      return .contested
  | "unavailable" =>
      let _ ← exactObject value ["reason", "status"]
      match ← text (← field value "reason") with
      | "unsupported_type" => return .unavailable .unsupportedType
      | "formula_value" => return .unavailable .formulaValue
      | "invalid_value" => return .unavailable .invalidValue
      | _ => malformed
  | _ => malformed

private def decodeMember (value : Json) : DecodeM Member := do
  let _ ← exactObject value ["fields", "id"]
  let .obj fields := (← field value "fields") | malformed
  let decoded ← fields.foldlM (init := []) fun result key item => do
    return (key, ← decodeCell item) :: result
  return { id := ← text (← field value "id"), fields := decoded.reverse }

private def decodeScope (value : Json) : DecodeM Scope := do
  let _ ← exactObject value ["fields", "id", "members", "witness"]
  let members ← array (← field value "members")
  return { id := ← text (← field value "id")
           fields := ← textArray (← field value "fields")
           witness := ← decodeScopeWitness (← field value "witness")
           members := ← members.toList.mapM decodeMember }

def decodeRequest (value : Json) : DecodeM Request := do
  let _ ← exactObject value ["operation", "request_id", "required_modules", "resources",
    "root_witness", "scope", "version"]
  let operationWrapper ← field value "operation"
  let _ ← exactObject operationWrapper ["query"]
  let queryValue ← field operationWrapper "query"
  let operation ← decodeOperation queryValue
  let queryScope ← text (← field queryValue "scope")
  let rootValue ← field value "root_witness"
  let root ← match rootValue with
    | .null => pure none
    | other => some <$> decodeNodeWitness other
  return { version := ← natural (← field value "version")
           requestId := ← text (← field value "request_id")
           resources := ← decodeResources (← field value "resources")
           requiredModules := ← textArray (← field value "required_modules")
           query := { version := ← natural (← field queryValue "version"),
                      scope := queryScope, operation }
           rootWitness := root
           scope := ← decodeScope (← field value "scope") }

private def jsonNat (value : Nat) : Json := .num value
private def jsonInt (value : Int) : Json := .num value

partial def encodeValue : Kpopper.Query.Value → Json
  | .scalar (.number number) => Json.mkObj [
      ("type", "number"), ("numerator", toString number.num), ("denominator", toString number.den)]
  | .scalar (.boolean value) => Json.mkObj [("type", "boolean"), ("value", value)]
  | .scalar (.text value) => Json.mkObj [("type", "text"), ("value", value)]
  | .scalar .null => Json.mkObj [("type", "null")]
  | .array values _ => Json.mkObj [("type", "list"), ("items", .arr (values.map encodeValue).toArray)]
  | .object fields _ => Json.mkObj [("type", "record"), ("fields", Json.mkObj (fields.map fun item =>
      (item.1, encodeValue item.2)))]

private def encodeLocation (value : Location) : Json := Json.mkObj [
  ("candidate", value.candidate), ("column", value.column), ("phase", value.phase.name)]

private def encodeDiagnostic (value : Diagnostic) : Json := Json.mkObj [
  ("code", value.code), ("locations", .arr (value.locations.map encodeLocation).toArray),
  ("related_ids", .arr (value.relatedIds.map Json.str).toArray)]

private def encodeScopeWitness (value : ScopeWitness) : Json := Json.mkObj [
  ("kind", "scope"), ("scope_id", value.scopeId),
  ("definition_digest", value.definitionDigest), ("membership_digest", value.membershipDigest),
  ("projected_inputs_digest", value.projectedInputsDigest)]

private def encodeNodeWitness (value : NodeWitness) : Json := Json.mkObj [
  ("kind", "node"), ("id", value.id), ("fingerprint", value.fingerprint)]

private def encodeWitness : Witness → Json
  | .node value => encodeNodeWitness value
  | .scope value => encodeScopeWitness value

private def encodeCounts (value : QueryCounts) : Json := Json.mkObj [
  ("input_count", jsonNat value.inputCount),
  ("definite_match_count", jsonNat value.definiteMatchCount),
  ("unknown_membership_count", jsonNat value.unknownMembershipCount),
  ("unknown_value_count", jsonNat value.unknownValueCount),
  ("error_count", jsonNat value.errorCount)]

private def encodeCost (value : Cost) : Json := Json.mkObj [
  ("steps", jsonNat value.steps), ("preflight_steps", jsonNat value.preflightSteps),
  ("node_evaluations", Json.mkObj (value.nodeEvaluations.map fun item => (item.1, jsonNat item.2))),
  ("candidates", jsonNat value.candidates), ("field_reads", jsonNat value.fieldReads),
  ("evaluated_field_reads", jsonNat value.evaluatedFieldReads)]

def encodeResponse (value : Response) : Json := Json.mkObj [
  ("version", jsonNat value.version),
  ("request_id", value.requestId.map Json.str |>.getD .null),
  ("status", value.status.name),
  ("value", value.value.map encodeValue |>.getD .null),
  ("diagnostics", .arr (value.diagnostics.map encodeDiagnostic).toArray),
  ("query_counts", value.queryCounts.map encodeCounts |>.getD .null),
  ("executed_reads", .arr (value.executedReads.map encodeWitness).toArray),
  ("cost", encodeCost value.cost)]

private def hexDigit (value : Nat) : Char :=
  "0123456789abcdef".toList.toArray[value % 16]!

private def escapeChar (result : String) (char : Char) : String :=
  if char == '"' then result ++ "\\\""
  else if char == '\\' then result ++ "\\\\"
  else if char == '\x08' then result ++ "\\b"
  else if char == '\x0c' then result ++ "\\f"
  else if char == '\n' then result ++ "\\n"
  else if char == '\r' then result ++ "\\r"
  else if char == '\t' then result ++ "\\t"
  else if char.toNat < 32 then
    result ++ "\\u" |>.push (hexDigit (char.toNat / 4096))
      |>.push (hexDigit (char.toNat / 256)) |>.push (hexDigit (char.toNat / 16))
      |>.push (hexDigit char.toNat)
  else result.push char

private def encodeString (value : String) : String :=
  "\"" ++ value.foldl escapeChar "" ++ "\""

partial def canonicalJson : Json → String
  | .null => "null"
  | .bool true => "true"
  | .bool false => "false"
  | .num value => toString value
  | .str value => encodeString value
  | .arr values => "[" ++ String.intercalate "," (values.toList.map canonicalJson) ++ "]"
  | .obj fields =>
      let items := fields.foldl (init := []) fun result key value =>
        (encodeString key ++ ":" ++ canonicalJson value) :: result
      "{" ++ String.intercalate "," items.reverse ++ "}"

private def wireFailure : Response := {
  requestId := none, status := .error,
  diagnostics := [{ code := "malformed_wire", relatedIds := [], locations := [] }] }

def responsePayload (payload : String) : String :=
  let response := match Json.parse payload with
    | .error _ => wireFailure
    | .ok value =>
        if canonicalJson value != payload then wireFailure
        else match decodeRequest value with
          | .error _ => wireFailure
          | .ok request => responseFor request
  canonicalJson (encodeResponse response)

def responseFrame (payload : String) : String :=
  let body := responsePayload payload
  "KR4 " ++ toString body.utf8ByteSize ++ "\n" ++ body

end Kpopper.Query.Wire
