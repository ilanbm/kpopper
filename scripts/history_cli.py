"""Inspect, reconcile, and rebuild recorded history; migrate only into a copy."""
import argparse
import json
from pathlib import Path

try:
    from . import provenance as P
except ImportError:
    import provenance as reader
    P = reader._peer('provenance')


def main(argv=None):
    parser = argparse.ArgumentParser(prog='kpopper history')
    parser.add_argument('operation', choices=('status', 'reconcile', 'rebuild', 'migrate', 'capabilities',
                                             'accept', 'refute', 'correct', 'propose', 'retire', 'adopt'))
    parser.add_argument('--record', help='record entry, using current project routing')
    parser.add_argument('--to', help='absent destination directory for a verified history copy')
    parser.add_argument('--read-mode', choices=('live', 'frozen'), help='capture live pending context or only the frozen record')
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--nonce', help='fresh correlation token for a runtime declaration')
    parser.add_argument('--subject', help='exact subject of an explicit history act')
    parser.add_argument('--of', dest='target', help='exact immutable claim version to act on')
    parser.add_argument('--over', action='append', default=[], help='exact claim version replaced; repeat as needed')
    parser.add_argument('--because', help='reason for the explicit act')
    parser.add_argument('--by', help='explicit recorded actor for adoption')
    parser.add_argument('--revision', help='retained pending contribution revision to adopt')
    parser.add_argument('--choose', action='append', default=[], help='SUBJECT=VERSION for every overlapping subject')
    parser.add_argument('--preview', action='store_true', help='show adoption choices without writing')
    parser.add_argument('--record-proposals', action='store_true', help='explicitly record current view edits as unaccepted proposals')
    parser.add_argument('--proposal-subject', action='append', help='subject included in edited-view proposals; repeat for all changed subjects')
    args = parser.parse_args(argv)
    try:
        if args.operation == 'capabilities':
            if args.record or args.to:
                raise ValueError('a runtime declaration does not select or migrate a record')
            print(json.dumps(P._peer('history_runtime').describe(args.nonce), ensure_ascii=False))
            return 0
        original_paths = [args.record] if args.record else P.default_paths()
        paths = P._peer('knowledge_views').write_paths(original_paths)
        if len(paths) != 1:
            raise ValueError('choose one logical record entry')
        entry = Path(paths[0]).absolute()
        if args.operation in ('accept', 'refute', 'correct', 'propose', 'retire'):
            if not args.subject or not args.target or not args.because:
                raise ValueError('explicit acts require --subject, --of, and --because')
            import contextlib
            import io
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                P.apply(original_paths, {'kind': args.operation, 'id': args.subject, 'of': args.target,
                                        'over': args.over, 'because': args.because})
            if output.getvalue().startswith('history committed:'):
                result = {'state': 'committed', 'act': args.operation, 'subject': args.subject, 'of': args.target}
            else:
                result = json.loads(output.getvalue())
        elif args.operation == 'adopt':
            if not args.revision:
                raise ValueError('adoption requires --revision')
            project = P._peer('knowledge_views').project_for(paths)
            choices = {}
            for choice in args.choose:
                if '=' not in choice:
                    raise ValueError('each --choose is SUBJECT=VERSION')
                subject, version = choice.split('=', 1)
                if subject in choices:
                    raise ValueError('a subject may be chosen only once')
                choices[subject] = version
            if args.preview:
                B, G = P._peer('history_bundle'), P._peer('pending_grounding')
                bundle = G.Store(project).snapshot()['bundles'].get(args.revision)
                if bundle is None:
                    raise ValueError('unknown contribution revision')
                result = B.preview_adoption(P._peer('history_store').Store(entry).capture(), B.from_contribution(bundle))
            else:
                if not args.by:
                    raise ValueError('adoption requires an explicit --by actor')
                result = P._peer('history_direct').adopt(paths, args.revision, choices,
                    by=args.by, project=project, original_paths=original_paths)
        elif args.operation == 'migrate':
            mode = args.read_mode or ('live' if P._peer('knowledge_views').project_for(paths).config()['mode'] == 'advanced'
                                      else 'frozen')
            plan = P._peer('history_migration').prepare(entry, route=False, read_mode=mode)
            result = plan.publish(args.to) if args.to else plan.summary()
        else:
            if args.to:
                raise ValueError('--to belongs to history migrate')
            store = P._peer('history_store').Store(entry)
            if args.operation == 'status':
                captured = store.capture()
                result = {'state': 'captured', 'record': str(entry), 'authority': captured.marker,
                          'commits': len(captured.commits), 'objects': len(captured.objects),
                          'subjects': {subject: {'acceptance': item['acceptance'], 'heads': item['heads']}
                                       for subject, item in captured.state['subjects'].items()}}
            elif args.operation == 'reconcile':
                if args.record_proposals:
                    if not args.because:
                        raise ValueError('recording proposals requires --because')
                    result = P._peer('history_direct').record_proposals(paths, args.proposal_subject,
                        because=args.because, by=args.by, project=P._peer('knowledge_views').project_for(paths),
                        original_paths=original_paths)
                else:
                    result = store.prepare_reconciliation(allow_conflicts=True)
            else:
                store.rebuild(write=True, allow_conflicts=True)
                captured = store.capture()
                result = {'state': 'rebuilt', 'record': str(entry), 'baseline': captured.baseline,
                          'unresolved_subjects': [subject for subject, item in captured.state['subjects'].items()
                                                   if item['acceptance'] != 'accepted']}
        print(json.dumps(result, ensure_ascii=False, default=str))
        return 0
    except (P.Refused, ValueError, OSError) as error:
        result = {'state': 'refused', 'code': getattr(error, 'code', 'history_refused'), 'detail': str(error)}
        print(json.dumps(result, ensure_ascii=False))
        return 1
