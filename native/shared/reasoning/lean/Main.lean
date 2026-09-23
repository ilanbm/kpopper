import CompositionProtocol
import Kpopper.QueryWire

partial def readExact (stream : IO.FS.Stream) (remaining : Nat)
    (acc : ByteArray := .empty) : IO (Option ByteArray) := do
  if remaining == 0 then return some acc
  let chunk ← stream.read remaining.toUSize
  if chunk.isEmpty then return none
  readExact stream (remaining - chunk.size) (acc ++ chunk)

def malformed4 : String := Kpopper.Query.Wire.responseFrame ""

/-- Native framing is ASCII; strings inside it are validated UTF-8 hex. -/
def main : IO Unit := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  repeat
    let line ← stdin.getLine
    if line.isEmpty then break
    let line := if line.endsWith "\n" then String.ofList (line.toList.dropLast) else line
    if line.startsWith "KP4 " then
      let lengthText := line.drop 4
      let some length := lengthText.toNat? | stdout.putStrLn malformed4; stdout.flush; continue
      if toString length != lengthText || length > 16777216 then
        stdout.putStrLn malformed4
        stdout.flush
        continue
      let some bytes ← readExact stdin length | stdout.putStrLn malformed4; stdout.flush; break
      let some terminator ← readExact stdin 1 | stdout.putStrLn malformed4; stdout.flush; break
      let payload := String.fromUTF8? bytes
      if terminator.size != 1 || terminator[0]! != 10 || payload.isNone then
        stdout.putStrLn malformed4
      else
        stdout.putStrLn (Kpopper.Query.Wire.responseFrame payload.get!)
    else
      stdout.putStrLn (if line == "KP3" || line.startsWith "KP3\t"
        then Kpopper.Composition.handle3 line else Kpopper.handle line)
    stdout.flush
