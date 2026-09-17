import CompositionProtocol

/-- Native framing is ASCII; strings inside it are validated UTF-8 hex. -/
def main : IO Unit := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  repeat
    let line ← stdin.getLine
    if line.isEmpty then break
    let line := if line.endsWith "\n" then String.ofList (line.toList.dropLast) else line
    stdout.putStrLn (if line == "KP3" || line.startsWith "KP3\t"
      then Kpopper.Composition.handle3 line else Kpopper.handle line)
    stdout.flush
