"""Measure the storage policy or the promise in the actual email template."""
from pathlib import Path
import re
import sys

if sys.argv[1] == "retention":
    text = Path("storage-policy.yaml").read_text()
    match = re.search(r"^retention_days:\s*(\d+)\s*$", text, re.MULTILINE)
    if match is None:
        raise ValueError("Expected an explicit retention_days: N line")
    days = int(match.group(1))
else:
    text = Path("download-email.html").read_text()
    matches = re.findall(r"Download available for (\d+) days", text)
    if len(matches) != 1:
        raise ValueError("Expected one explicit download duration")
    days = int(matches[0])
if type(days) is not int or days < 1:
    raise ValueError("Expected a positive whole number of days")
print(days)
