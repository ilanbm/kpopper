from pathlib import Path
import yaml


def retention_days():
    return yaml.safe_load(Path("storage-policy.yaml").read_text())["retention_days"]


def available(age_days):
    return age_days < retention_days()


def purge_exports(directory, now):
    """In this small example, an export's mtime is its creation time."""
    for path in Path(directory).iterdir():
        if path.is_file() and not available((now - path.stat().st_mtime) / 86400):
            path.unlink()
