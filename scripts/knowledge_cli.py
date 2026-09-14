"""Explicit local contribution status, portable snapshots and legacy import."""
import argparse
import json
import os
from pathlib import Path
import uuid

try:
    from . import knowledge_views as V, pending_grounding as G, recording as R
except ImportError:
    import knowledge_views as V
    import pending_grounding as G
    import recording as R


def main(argv):
    parser = argparse.ArgumentParser(prog='kpopper knowledge')
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('status')
    export = sub.add_parser('materialize')
    export.add_argument('revision')
    export.add_argument('--out', required=True)
    export.add_argument('--ref')
    imp = sub.add_parser('import', help='explicitly import an old shared record without changing its files/configuration')
    imp.add_argument('file')
    imp.add_argument('--shareability', choices=('project', 'private', 'unclear'), required=True)
    imp.add_argument('--scope', choices=('project', 'external'), required=True)
    imp.add_argument('--environment', required=True)
    imp.add_argument('--evidence', help='root of permitted portable evidence, explicitly allowed for this import')
    imp.add_argument('--event-id')
    args = parser.parse_args(argv)
    try:
        project = G.M.Project()
        if args.command == 'materialize':
            result = V.materialize(project, args.revision, args.out, ref=args.ref)
        elif args.command == 'status':
            path = project.record()
            doc = G.P.load([str(path)]) if path.exists() else V.overlay(
                [str(path)], G.P.Record(), read_mode=os.environ.get('KPOPPER_READ_MODE', 'live'))
            result = {'mode': project.config()['mode'], 'record': str(path),
                      'read_mode': getattr(doc, 'read_mode', 'live'),
                      'contributions': getattr(doc, 'contributions', []),
                      'conflicts': getattr(doc, 'knowledge_conflicts', {}),
                      'publication': getattr(doc, 'publication', None),
                      'private_drafts': getattr(doc, 'private_drafts', []),
                      'target_unavailable': getattr(doc, 'target_unavailable', None)}
        else:
            doc = G.P.load([str(Path(args.file).resolve())], read_mode='frozen')
            scope = {'kind': args.scope, 'environment': args.environment}
            action = {'kind': 'import', 'shareability': args.shareability, 'scope': scope,
                      'event_id': args.event_id or uuid.uuid4().hex}
            if args.shareability != 'project' or R.private_marker(doc) or R.private_locator(doc):
                result = R.draft(project, action, doc, 'private or unclear import permission')
            else:
                files = {}
                for name in G._files(doc):
                    G.M.relative_path(name)
                    if not args.evidence:
                        raise ValueError('import needs an explicit --evidence root for referenced files')
                    root = Path(args.evidence).resolve()
                    path = (root / name).resolve()
                    path.relative_to(root)
                    files[name] = path.read_bytes()
                entries = G.entries(doc)
                cited = {source for _, body in entries.values() if isinstance(body, dict)
                         for source in ([body.get('from')] if isinstance(body.get('from'), str)
                                        else body.get('from', []) if isinstance(body.get('from'), list) else [])
                         if source in entries}
                roots = sorted(set(entries) - cited)
                bundle = G.prepare(doc, roots, scope=scope,
                                   shareability='project', evidence=files)
                result = G.Store(project).capture(bundle, event_id=action['event_id'],
                    contribution_id='import-' + bundle['revision'], shareability='project')
        print(json.dumps(result, ensure_ascii=False, default=str))
        return 0
    except (OSError, ValueError) as error:
        print(json.dumps({'state': 'needs attention', 'error': str(error)}))
        return 2
