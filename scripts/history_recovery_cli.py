"""Recover an existing prepared direct operation without preparing a new write."""
import argparse
import json

try:
    from . import provenance as P
except ImportError:
    import provenance as P


def main(argv=None):
    parser = argparse.ArgumentParser(prog='kpopper recover')
    parser.add_argument('--record', help='entry file, routed through the current project configuration')
    parser.add_argument('--rollback', action='store_true', help='restore the retained before images')
    parser.add_argument('--json', action='store_true', help='return a structured receipt')
    args = parser.parse_args(argv)
    try:
        paths = [args.record] if args.record else P.default_paths()
        mutation = P.recover_direct(paths, direction='before' if args.rollback else 'after')
        if isinstance(mutation, dict):
            print(json.dumps(mutation, ensure_ascii=False, default=str))
            return 0 if mutation.get('state') in ('applied', 'needs_primary') else 1
        data = mutation.to_data()
        result = {'state': 'restored' if args.rollback else 'recovered',
                  'operation': data['operation'], 'mutation_digest': data['digest']}
        print(json.dumps(result, ensure_ascii=False) if args.json else
              result['state'] + ': ' + result['operation'])
        return 0
    except (P.Refused, ValueError, OSError) as error:
        result = {'state': 'refused', 'code': getattr(error, 'code', 'recovery_refused'),
                  'detail': str(error)}
        print(json.dumps(result, ensure_ascii=False) if args.json else 'refused: ' + str(error))
        return 1
