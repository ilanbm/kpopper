import Kpopper.Query

/-! Pure KP4 request-to-KR4 response binding. Canonical JSON parsing and the
outer mixed KP2/KP3/KP4 stream dispatcher intentionally live outside this
module; malformed bytes never reach `responseFor`. -/
namespace Kpopper.Query

private def digit (c : Char) : Bool := c ≥ '0' && c ≤ '9'
private def lowerHex (c : Char) : Bool := digit c || (c ≥ 'a' && c ≤ 'f')
private def digest (value : String) : Bool := value.length == 64 && value.toList.all lowerHex

private def modulesEqual (a b : List String) : Bool :=
  a.length == b.length && (a.zip b).all fun pair => pair.1 == pair.2

def limitCode (code : String) : Bool :=
  (["candidate_limit", "field_read_limit", "depth_limit", "value_limit", "digit_limit",
    "step_limit"] : List String).contains code

def statusForCode (code : String) : Status :=
  if code == "unsupported_capability" then .unsupportedCapability
  else if limitCode code then .limit
  else .error

def preflightDiagnostic (request : Request) (code : String) : Diagnostic :=
  { code,
    relatedIds := if request.scope.id.isEmpty then [] else [request.scope.id],
    locations := [] }

def resourcesValid (resources : Resources) : Bool :=
  resources.version == "resources/v4" &&
    resources.steps > 0 && resources.steps ≤ 1000000 &&
    resources.depth > 0 && resources.depth ≤ 128 &&
    resources.digits > 0 && resources.digits ≤ 256 &&
    resources.valueNodes > 0 && resources.valueNodes ≤ 10000 &&
    resources.valueDepth > 0 && resources.valueDepth ≤ 128 &&
    resources.valueBytes > 0 && resources.valueBytes ≤ 16777216 &&
    resources.candidates > 0 && resources.candidates ≤ 10000 &&
    resources.fieldReads > 0 && resources.fieldReads ≤ 100000

def witnessesValid (request : Request) : Bool :=
  request.scope.witness.scopeId == request.scope.id &&
    digest request.scope.witness.definitionDigest &&
    digest request.scope.witness.membershipDigest &&
    digest request.scope.witness.projectedInputsDigest &&
    match request.rootWitness with
    | none => true
    | some root => !root.id.isEmpty && digest root.fingerprint

def validateRequest (request : Request) : Except String Prepared := do
  if request.version != 4 || !digest request.requestId then throw "invalid_expression"
  if !resourcesValid request.resources then throw "invalid_limits"
  if !witnessesValid request then throw "invalid_expression"
  if request.query.version != 1 || request.query.scope != request.scope.id then throw "invalid_expression"
  if !modulesEqual request.requiredModules (requiredModules request.query.operation) then
    throw "unsupported_capability"
  prepare request.resources request.scope request.query.operation

def actualWitnesses (request : Request) : List Witness :=
  (request.rootWitness.map (fun value => Witness.node value)).toList ++
    [Witness.scope request.scope.witness]

def refusal (request : Request) (code : String) (cost : Cost := {}) : Response :=
  { requestId := some request.requestId,
    status := statusForCode code,
    diagnostics := [preflightDiagnostic request code],
    executedReads := actualWitnesses request,
    cost }

def responseFor (request : Request) : Response :=
  match validateRequest request with
  | .error code => refusal request code
  | .ok prepared =>
    let baseCost : Cost := {
      preflightSteps := prepared.preflightSteps,
      nodeEvaluations := match request.rootWitness with
        | none => []
        | some root => [(root.id, 1)],
      candidates := prepared.candidates,
      fieldReads := prepared.fieldReads }
    match execute request.resources request.scope request.query.operation with
    | .error code => refusal request code baseCost
    | .ok completed => {
        requestId := some request.requestId,
        status := completed.status,
        value := completed.result,
        diagnostics := completed.diagnostics,
        queryCounts := some completed.counts,
        executedReads := actualWitnesses request,
        cost := {
          steps := completed.steps,
          preflightSteps := baseCost.preflightSteps,
          nodeEvaluations := baseCost.nodeEvaluations,
          candidates := baseCost.candidates,
          fieldReads := baseCost.fieldReads,
          evaluatedFieldReads := completed.evaluatedFieldReads } }

/-! A deterministic, deliberately non-wire summary for standalone kernel tests.
It cannot be confused with KR4 framing and is not used by the production
dispatcher. -/
def renderValueFuel : Nat → Value → String
  | 0, _ => "<depth>"
  | _ + 1, .scalar (.number number) => "n:" ++ toString number.num ++ "/" ++ toString number.den
  | _ + 1, .scalar (.boolean value) => if value then "b:true" else "b:false"
  | _ + 1, .scalar (.text value) => "s:" ++ value
  | _ + 1, .scalar .null => "null"
  | fuel + 1, .array values _ =>
    "[" ++ String.intercalate "," (values.map (renderValueFuel fuel)) ++ "]"
  | fuel + 1, .object fields _ => "{" ++ String.intercalate "," (fields.map fun item =>
      item.1 ++ "=" ++ renderValueFuel fuel item.2) ++ "}"

def renderValue (value : Value) : String := renderValueFuel 130 value

def renderLocation (location : Location) : String :=
  location.candidate ++ "/" ++ location.column ++ "/" ++ location.phase.name

def renderDiagnostic (diagnostic : Diagnostic) : String :=
  diagnostic.code ++ "@" ++ String.intercalate "," (diagnostic.locations.map renderLocation)

def renderCounts : Option QueryCounts → String
  | none => "-"
  | some counts => String.intercalate "," [
      toString counts.inputCount,
      toString counts.definiteMatchCount,
      toString counts.unknownMembershipCount,
      toString counts.unknownValueCount,
      toString counts.errorCount]

def renderResponse (response : Response) : String :=
  String.intercalate "\t" [
    response.status.name,
    renderCounts response.queryCounts,
    (response.value.map renderValue).getD "-",
    String.intercalate ";" (response.diagnostics.map renderDiagnostic),
    toString response.cost.steps,
    toString response.cost.preflightSteps,
    toString response.cost.candidates,
    toString response.cost.fieldReads,
    toString response.cost.evaluatedFieldReads]

def runRequest (request : Request) : String := renderResponse (responseFor request)

end Kpopper.Query
