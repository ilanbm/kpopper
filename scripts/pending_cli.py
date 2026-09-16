"""Inspect and manage publication for the project's durable contributions."""
import argparse
import json
import subprocess
from pathlib import Path

try:
    from . import project_modes as M
    from . import pending_publication as U
except ImportError:
    import project_modes as M
    import pending_publication as U


def parser():
    result = argparse.ArgumentParser(prog='kpop pending', description=__doc__)
    result.add_argument('--json', action='store_true', help='return structured state')
    commands = result.add_subparsers(dest='command', required=True)
    status = commands.add_parser('status', help='inspect local capture and publication state')
    status.add_argument('--verify', action='store_true', help='freshly check target incorporation without publishing')
    config = commands.add_parser('configure', help='retain the exact publication scope and permission')
    config.add_argument('--remote', help='configured Git remote name')
    config.add_argument('--target', help='review target branch')
    config.add_argument('--branch', help='managed head branch (default: pending_grounding)')
    grant = config.add_mutually_exclusive_group()
    grant.add_argument('--grant', action='store_true', help='grant standing push and PR creation/update permission for this scope')
    grant.add_argument('--revoke', action='store_true', help='revoke standing publication permission')
    publish = commands.add_parser('publish', help='reconcile and attempt one publication cycle')
    publish.add_argument('--authorize', action='store_true', help='authorize this publication attempt for the configured scope')
    publish.add_argument('--retry', action='store_true', help='explicitly retry after bounded backoff')
    for action in ('pause', 'resume', 'withdraw', 'reject', 'supersede', 'retry'):
        command = commands.add_parser(action, help='record an explicit ' + action + ' decision')
        command.add_argument('revisions', nargs='*', help='exact immutable contribution revisions')
        command.add_argument('--reason', default='', help='why this decision was made')
        if action == 'supersede':
            command.add_argument('--replacement', required=True, help='captured revision replacing the selected ones')
    return result


def configure(project, args):
    previous = project.config().get('publication') or {}
    remote = args.remote or previous.get('remote')
    target = args.target or previous.get('target')
    branch = args.branch or previous.get('branch') or 'pending_grounding'
    if not remote or not target:
        raise ValueError('first publication configuration needs --remote and --target')
    # A changed scope receives no inherited permission. Project enforces its own
    # reconciliation boundary before allowing an existing ledger to be redirected.
    same = all(previous.get(key) == value for key, value in
               (('remote', remote), ('target', target), ('branch', branch)))
    urls = M.git(project.root, 'remote', 'get-url', '--push', '--all', remote).stdout.decode().splitlines()
    same = same and urls == [previous.get('repository')]
    grant = args.grant or (same and previous.get('standing_permission', False) and not args.revoke)
    scope = project.publication_config(remote, target, branch=branch, grant=bool(grant))
    return {'publication': scope, 'published': False}


def main(argv=None):
    import sys
    argv = list(sys.argv[1:] if argv is None else argv)
    # Accept --json at either command boundary, as the top-level CLI does.
    as_json = '--json' in argv
    args = parser().parse_args([x for x in argv if x != '--json'])
    try:
        project = M.Project(Path.cwd())
        if args.command == 'configure':
            value = configure(project, args)
        elif not project.git:
            if args.command != 'status':
                raise ValueError('Simple projects without Git have no publication queue')
            value = {'mode': 'simple', 'states': {}, 'verified': False,
                     'publication': None, 'reason': 'Simple uses the shared project record'}
        else:
            publisher = U.Publisher(project)
            if args.command == 'status':
                value = publisher.status()
                if args.verify:
                    value['verification'] = publisher.verify_obligations()
                value['authority'] = publisher.authority()
                value['mode'] = project.config()['mode']
            elif args.command == 'publish':
                value = publisher.run(authorized=args.authorize, force_retry=args.retry)
            else:
                value = publisher.action(args.command, revisions=args.revisions,
                                         reason=args.reason, replacement=getattr(args, 'replacement', None))
        # The state is structured by default too: printing it never upgrades a
        # cached observation into a verified claim or silently loses its scope.
        print(json.dumps(value, ensure_ascii=False, indent=2, default=str))
        return 0
    except (ValueError, OSError, RuntimeError, subprocess.SubprocessError) as error:
        if as_json:
            print(json.dumps({'error': str(error)}, ensure_ascii=False))
        else:
            print('kpop pending: ' + str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
