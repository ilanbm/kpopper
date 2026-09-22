"""Record the files a CI lane opens and check them against the inputs it declares.

`record` runs as root on Linux and watches every open through fanotify, which adds no
measurable time to the traced steps. `check` maps what was opened back to tracked files and
fails when the lane read a file, or listed a directory, that `ci_selection.LANES` does not
declare for it: that is the dependency a pull request would otherwise skip.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import sys
import time

FAN_CLOEXEC, FAN_NONBLOCK, FAN_UNLIMITED_QUEUE = 0x1, 0x2, 0x10
FAN_MARK_ADD, FAN_MARK_FILESYSTEM = 0x1, 0x100
FAN_OPEN, FAN_Q_OVERFLOW, FAN_ONDIR = 0x20, 0x4000, 0x40000000
AT_FDCWD = -100
METADATA = struct.Struct('=IBBHQii')
PYC = re.compile(r'(?:^|/)__pycache__/([^/.]+)\.[^/]*\.pyc$')


def record(output, ready, prefixes):
    """Collect every opened path under the prefixes until SIGTERM, then write them once."""
    libc = ctypes.CDLL(None, use_errno=True)
    libc.fanotify_mark.argtypes = [ctypes.c_int, ctypes.c_uint, ctypes.c_uint64, ctypes.c_int, ctypes.c_char_p]
    flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, 'O_LARGEFILE', 0)
    fd = libc.fanotify_init(FAN_CLOEXEC | FAN_NONBLOCK | FAN_UNLIMITED_QUEUE, flags)
    if fd < 0:
        raise OSError(ctypes.get_errno(), 'fanotify_init failed')
    for prefix in prefixes:
        if libc.fanotify_mark(fd, FAN_MARK_ADD | FAN_MARK_FILESYSTEM, FAN_OPEN | FAN_ONDIR,
                              AT_FDCWD, os.fsencode(prefix)) != 0:
            raise OSError(ctypes.get_errno(), 'fanotify_mark failed for ' + prefix)
    stopping = []
    signal.signal(signal.SIGTERM, lambda *_: stopping.append(time.monotonic()))
    seen, events, overflow, me = set(), 0, False, os.getpid()
    Path(ready).write_text(str(me) + '\n')
    while True:
        # After SIGTERM, drain what was queued before it (bounded, the system never goes quiet).
        if stopping and time.monotonic() - stopping[0] > 5:
            break
        readable, _, _ = select.select([fd], [], [], 0.2)
        if not readable:
            if stopping:
                break
            continue
        try:
            data = os.read(fd, 1 << 16)
        except BlockingIOError:
            continue
        offset = 0
        while offset + METADATA.size <= len(data):
            length, _, _, _, mask, event_fd, pid = METADATA.unpack_from(data, offset)
            offset += length
            if mask & FAN_Q_OVERFLOW:
                overflow = True
            if event_fd < 0:
                continue
            try:
                if pid != me:
                    path = os.readlink('/proc/self/fd/%d' % event_fd)
                    if path.startswith(tuple(prefixes)):
                        seen.add(path)
                        events += 1
            except OSError:
                pass
            finally:
                os.close(event_fd)
    Path(output).write_text(json.dumps({'overflow': overflow, 'events': events, 'paths': sorted(seen)}) + '\n')


def tracked_paths(root):
    raw = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root)
    files = {p.decode('utf-8', 'surrogateescape') for p in raw.split(b'\0') if p}
    directories = {''} | {p.rsplit('/', 1)[0] for p in files if '/' in p}
    for directory in list(directories):
        while '/' in directory:
            directory = directory.rsplit('/', 1)[0]
            directories.add(directory)
    return files, directories


def repository_path(path, root, packages):
    """The tracked path an opened file stands for: the checkout itself or an installed copy."""
    match = PYC.search(path)
    if match:
        path = path[:match.start()] + ('/' if match.group(0).startswith('/') else '') + match.group(1) + '.py'
    for prefix, source in [(root, '')] + list(packages):
        prefix = prefix.rstrip('/')
        if path == prefix:
            return source.rstrip('/')
        if path.startswith(prefix + '/'):
            return source + path[len(prefix) + 1:]
    return None


def undeclared(lane, opened, root, packages, ignore=(), lanes=None):
    """Tracked files and directories the lane opened outside its declared inputs."""
    from ci_selection import LANES, lane_reads, lane_lists
    lanes = lanes or LANES
    files, directories = tracked_paths(root)
    missing_files, missing_dirs = set(), set()
    for path in opened:
        rel = repository_path(path, root, packages)
        if rel is None or rel == '.git' or rel.startswith('.git/') or path in ignore:
            continue
        if rel in files and not lane_reads(lanes[lane], rel):
            missing_files.add(rel)
        elif rel in directories and rel not in files and not lane_lists(lanes[lane], rel):
            missing_dirs.add(rel)
    return sorted(missing_files), sorted(missing_dirs)


def check(lane, trace, root, packages, canary, lanes=None):
    from ci_selection import LANES
    enforced = (lanes or LANES)[lane].enforced
    data = json.loads(Path(trace).read_text(encoding='utf-8'))
    problems = []
    if data.get('overflow'):
        problems.append('the recorder lost events (fanotify queue overflow); the audit cannot vouch for this run')
    canary = str(Path(canary).resolve()) if canary else None
    if canary and canary not in data['paths']:
        problems.append('the recorder did not see the canary read of ' + canary + '; it was not watching')
    files, directories = undeclared(lane, data['paths'], root, packages, ignore={canary}, lanes=lanes)
    for path in files:
        problems.append('%s read %s, which its inputs in .github/scripts/ci_selection.py do not list' % (lane, path))
    for path in directories:
        problems.append('%s listed the directory %s, which its inputs do not list; adding or removing a file there'
                        ' could change its result' % (lane, path or '(repository root)'))
    print(json.dumps({'lane': lane, 'events': data.get('events'), 'opened': len(data['paths']),
                      'undeclared_files': files, 'undeclared_directories': directories}, indent=2))
    for problem in problems:
        print(('::error::' if enforced else '::warning::') + problem)
    return 1 if problems and enforced else 0


def state_dir():
    return Path(os.environ.get('RUNNER_TEMP') or '/tmp') / 'kpopper-read-audit'


def installed_packages():
    """Where this interpreter installs the package, and the tracked directory it copies."""
    import sysconfig
    return [(str(Path(sysconfig.get_paths()['purelib']) / 'kpopper'), 'scripts/')]


def start(root):
    """Launch the recorder as root in its own session and wait until it is watching."""
    state = state_dir()
    state.mkdir(parents=True, exist_ok=True)
    ready, output = state / 'ready', state / 'reads.json'
    for stale in (ready, output):
        if stale.exists():
            raise SystemExit('a read audit already ran in ' + str(state))
    prefixes = [root] + [installed for installed, _ in installed_packages()]
    # Tests that run git in the checkout would otherwise re-read every file whose index entry
    # is racily clean after checkout, which is not a dependency of anything. A refresh a second
    # after the last write rewrites the index with a newer timestamp than every entry.
    time.sleep(1.1)
    subprocess.run(['git', 'update-index', '-q', '--refresh'], cwd=root, check=False)
    with (state / 'recorder.log').open('w') as log:
        subprocess.Popen(['sudo', '-n', sys.executable, os.path.abspath(__file__), 'record',
                          '--output', str(output), '--ready', str(ready)]
                         + [arg for prefix in prefixes for arg in ('--prefix', prefix)],
                         stdout=log, stderr=log, start_new_session=True)
    deadline = time.monotonic() + 60
    while not (ready.exists() and ready.read_text().strip()):
        if time.monotonic() > deadline:
            raise SystemExit('the file-read recorder did not start:\n' + (state / 'recorder.log').read_text())
        time.sleep(0.2)
    print('recording file reads under ' + ', '.join(prefixes))
    return 0


def finish(lane, root):
    """Stop the recorder after a read it must see, then check the lane's reads."""
    state = state_dir()
    canary = Path(root) / '.github/scripts/ci_audit.py'
    canary.read_bytes()
    pid = (state / 'ready').read_text().strip()
    subprocess.run(['sudo', '-n', sys.executable, '-c', 'import os, signal, sys; os.kill(int(sys.argv[1]), signal.SIGTERM)',
                    pid], check=True)
    output, deadline = state / 'reads.json', time.monotonic() + 120
    while not output.exists():
        if time.monotonic() > deadline:
            raise SystemExit('the file-read recorder did not finish:\n' + (state / 'recorder.log').read_text())
        time.sleep(0.2)
    time.sleep(0.5)
    return check(lane, output, root, installed_packages(), canary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('start')
    fin = commands.add_parser('finish')
    fin.add_argument('--lane', required=True)
    rec = commands.add_parser('record')
    rec.add_argument('--output', required=True)
    rec.add_argument('--ready', required=True)
    rec.add_argument('--prefix', action='append', required=True)
    chk = commands.add_parser('check')
    chk.add_argument('--lane', required=True)
    chk.add_argument('--trace', required=True)
    chk.add_argument('--root', default='.')
    chk.add_argument('--package', action='append', default=[],
                     help='INSTALLED=SOURCE: an installed copy of a tracked directory')
    chk.add_argument('--canary')
    args = parser.parse_args()
    if args.command == 'record':
        record(args.output, args.ready, [str(Path(p).resolve()) for p in args.prefix])
        return 0
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    workspace = str(Path(os.environ.get('GITHUB_WORKSPACE') or '.').resolve())
    if args.command == 'start':
        return start(workspace)
    if args.command == 'finish':
        return finish(args.lane, workspace)
    root = str(Path(args.root).resolve())
    packages = []
    for item in args.package:
        installed, _, source = item.partition('=')
        packages.append((str(Path(installed).resolve()), source.rstrip('/') + '/'))
    return check(args.lane, args.trace, root, packages, args.canary)


if __name__ == '__main__':
    raise SystemExit(main())
