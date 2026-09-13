"""Measure the storage policy or the promise in the actual email template."""
from pathlib import Path
import re
import sys
import yaml

if sys.argv[1] == "retention":
    days = yaml.safe_load(Path("storage-policy.yaml").read_text())["retention_days"]
else:
    text = Path("download-email.html").read_text()
    matches = re.findall(r"Download available for (\d+) days", text)
    if len(matches) != 1:
        raise ValueError("Expected one explicit download duration")
    days = int(matches[0])
if type(days) is not int or days < 1:
    raise ValueError("Expected a positive whole number of days")
print(days)
