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
    parser.add_argument('operation', choices=('status', 'reconcile', 'rebuild', 'migrate', 'capabilities'))
    parser.add_argument('--record', help='record entry, using current project routing')
    parser.add_argument('--to', help='absent destination directory for a verified history copy')
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--nonce', help='fresh correlation token for a runtime declaration')
    args = parser.parse_args(argv)
    try:
        if args.operation == 'capabilities':
            if args.record or args.to:
                raise ValueError('a runtime declaration does not select or migrate a record')
            print(json.dumps(P._peer('history_runtime').describe(args.nonce), ensure_ascii=False))
            return 0
        paths = [args.record] if args.record else P.default_paths()
        paths = P._peer('knowledge_views').write_paths(paths)
        if len(paths) != 1:
            raise ValueError('choose one logical record entry')
        entry = Path(paths[0]).absolute()
        if args.operation == 'migrate':
            plan = P._peer('history_migration').prepare(entry, route=False, read_mode='frozen')
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
