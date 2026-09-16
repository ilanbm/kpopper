import Kernel
import Init.Data.Rat.Lemmas

/-! This proof concerns the actual production evaluator in Kernel. It certifies
successful closed rational arithmetic only; parser, graph traversal, type
inference, protocol, native generation, and source-world truth are outside it. -/
namespace Kpopper.Proof

inductive ArithOp where
  | add | sub | mul | div

def ArithOp.toOp : ArithOp → Op
  | .add => .add | .sub => .sub | .mul => .mul | .div => .div

inductive ClosedRat : Expr → Prop where
  | literal (r : Rat) : ClosedRat (.literal (.number r))
  | binary {left right : Expr} (op : ArithOp) :
      ClosedRat left → ClosedRat right → ClosedRat (.binary op.toOp left right)

/-- Independent arithmetic denotation, including the nonzero division premise. -/
inductive ArithmeticDenotes : ArithOp → Rat → Rat → Rat → Prop where
  | add (x y : Rat) : ArithmeticDenotes .add x y (x + y)
  | sub (x y : Rat) : ArithmeticDenotes .sub x y (x - y)
  | mul (x y : Rat) : ArithmeticDenotes .mul x y (x * y)
  | div (x y : Rat) : y ≠ 0 → ArithmeticDenotes .div x y (x / y)

inductive Denotes : Expr → Rat → Prop where
  | literal (r : Rat) : Denotes (.literal (.number r)) r
  | binary {left right : Expr} {op : ArithOp} {x y z : Rat} :
      Denotes left x → Denotes right y → ArithmeticDenotes op x y z →
      Denotes (.binary op.toOp left right) z

theorem bind_run {α β : Type} (m : EvalM α) (f : α → EvalM β) (st : State) :
    ((m >>= f).run).run st =
      match (m.run).run st with
      | (.error err, state) => (.error err, state)
      | (.ok value, state) => ((f value).run).run state := by
  cases h : (m.run).run st with
  | mk result state =>
    cases result <;>
      simp [ExceptT.run, StateT.run, bind, ExceptT.bind, StateT.bind,
        ExceptT.mk, ExceptT.bindCont, pure] at h ⊢ <;>
      simp_all <;> rfl

theorem numeric_success (x : Rat) (limits : Limits) (st st' : State) (r : Rat)
    (h : ((numeric x limits).run).run st = (.ok (.known (.number r)), st')) :
    r = x ∧ st' = st := by
  unfold numeric at h
  split at h
  · have impossible := congrArg Prod.fst h
    cases impossible
  · change (Except.ok (Result.known (Value.number x)), st) =
      (Except.ok (Result.known (Value.number r)), st') at h
    cases h
    exact ⟨rfl, rfl⟩

theorem binary_sound (op : ArithOp) (a b : Result) (limits : Limits)
    (st st' : State) (r : Rat)
    (h : ((binary op.toOp a b limits).run).run st = (.ok (.known (.number r)), st')) :
    ∃ x y, a = .known (.number x) ∧ b = .known (.number y) ∧
      ArithmeticDenotes op x y r ∧ st' = st := by
  cases op
  all_goals cases a <;> try (cases (by assumption : Value))
  all_goals cases b <;> try (cases (by assumption : Value))
  all_goals dsimp [ArithOp.toOp, binary, arithmetic, badNumeric, unavailable] at h
  all_goals try simp at h
  all_goals try (solve | have impossible := congrArg Prod.fst h; cases impossible)
  all_goals try (split at h)
  all_goals try (solve | have impossible := congrArg Prod.fst h; cases impossible)
  all_goals try (split at h)
  all_goals try (solve | have impossible := congrArg Prod.fst h; cases impossible)
  all_goals
    obtain ⟨hr, hs⟩ := numeric_success _ limits st st' r h
    subst r
    first
    | exact ⟨_, _, rfl, rfl, .add _ _, hs⟩
    | exact ⟨_, _, rfl, rfl, .sub _ _, hs⟩
    | exact ⟨_, _, rfl, rfl, .mul _ _, hs⟩
    | exact ⟨_, _, rfl, rfl, .div _ _ (by assumption), hs⟩

@[simp] theorem get_run (st : State) :
    ((get : EvalM State).run).run st = (.ok st, st) := rfl

@[simp] theorem modify_run (f : State → State) (st : State) :
    ((modify f : EvalM Unit).run).run st = (.ok (), f st) := rfl

theorem evaluate_closedRat_sound
    (fuel : Nat) (nodes : Std.HashMap String Expr) (limits : Limits)
    (depth : Nat) (e : Expr) (closed : ClosedRat e) (st st' : State) (r : Rat)
    (h : ((evaluate fuel nodes limits depth e).run).run st =
      (.ok (.known (.number r)), st')) :
    Denotes e r ∧ st'.memo = st.memo ∧ st'.reads = st.reads := by
  induction fuel generalizing depth e st st' r with
  | zero =>
    have impossible := congrArg Prod.fst h
    cases impossible
  | succ fuel ih =>
    rw [evaluate.eq_def] at h
    dsimp only at h
    split at h
    · have impossible := congrArg Prod.fst h
      cases impossible
    · simp only [bind_run, get_run] at h
      split at h
      · have impossible := congrArg Prod.fst h
        cases impossible
      · simp only [bind_run, modify_run] at h
        cases closed with
        | literal x =>
          dsimp only at h
          obtain ⟨hr, hs⟩ := numeric_success x limits _ st' r h
          subst r
          subst st'
          exact ⟨.literal x, rfl, rfl⟩
        | @binary left right op cl cr =>
          dsimp only at h
          rw [bind_run] at h
          cases hl : ((evaluate fuel nodes limits (depth + 1) left).run).run
              {st with steps := st.steps + 1} with
          | mk al st1 =>
            rw [hl] at h
            cases al with
            | error why =>
              have impossible := congrArg Prod.fst h
              cases impossible
            | ok a =>
              dsimp only at h
              rw [bind_run] at h
              cases hr : ((evaluate fuel nodes limits (depth + 1) right).run).run st1 with
              | mk br st2 =>
                rw [hr] at h
                cases br with
                | error why =>
                  have impossible := congrArg Prod.fst h
                  cases impossible
                | ok b =>
                  dsimp only at h
                  obtain ⟨x, y, ha, hb, ar, hs⟩ := binary_sound op a b limits st2 st' r h
                  subst a
                  subst b
                  obtain ⟨dl, ml, rl⟩ := ih (depth + 1) left cl _ st1 x hl
                  obtain ⟨dr, mr, rr⟩ := ih (depth + 1) right cr st1 st2 y hr
                  exact ⟨.binary dl dr ar, hs ▸ mr.trans ml, hs ▸ rr.trans rl⟩

/-- A non-vacuous success guarantee for every representable rational literal. -/
theorem evaluate_literal_success
    (fuel : Nat) (nodes : Std.HashMap String Expr) (limits : Limits)
    (depth : Nat) (st : State) (r : Rat)
    (hd : depth ≤ limits.depth) (hs : st.steps < limits.steps)
    (hn : (toString r.num.natAbs).length ≤ limits.digits)
    (hq : (toString r.den).length ≤ limits.digits) :
    ((evaluate (fuel + 1) nodes limits depth (.literal (.number r))).run).run st =
      (.ok (.known (.number r)), { st with steps := st.steps + 1 }) := by
  rw [evaluate.eq_def]
  simp only [Nat.not_lt_of_ge hd, ↓reduceIte, bind_run, get_run]
  simp only [Nat.not_le_of_gt hs, ↓reduceIte, bind_run, modify_run]
  unfold numeric
  have hbound : ¬((toString r.num.natAbs).length > limits.digits ∨
      (toString r.den).length > limits.digits) := by
    exact not_or_intro (Nat.not_lt_of_ge hn) (Nat.not_lt_of_ge hq)
  simp only [Bool.or_eq_true, decide_eq_true_eq, hbound, ↓reduceIte]
  rfl

end Kpopper.Proof
