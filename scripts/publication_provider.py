"""Pull-request transport. Git transport and authority live in pending_publication.

The provider must return a complete current list for the exact head/base scope or
raise UnknownOutcome. It must never turn failed reads into an empty list. Tests
use an explicitly identified provider fixture, never a live hosted repository.
"""
import json
import re
import subprocess
from urllib.parse import urlparse


class UnknownOutcome(RuntimeError):
    """The remote result cannot be established; reconcile before another write."""


class ProviderConflict(ValueError):
    pass


class GitHubProvider:
    """GitHub's authenticated CLI; no permission is inferred from authentication."""
    def __init__(self, repository):
        match = re.fullmatch(r'git@([^:]+):([^/]+/[^/]+?)(?:\.git)?', repository)
        if match:
            host, name = match.groups()
        else:
            parsed = urlparse(repository)
            if parsed.scheme not in ('https', 'ssh') or parsed.password or parsed.query or parsed.fragment:
                raise ProviderConflict('configure a supported GitHub repository URL')
            host, name = parsed.hostname, parsed.path.strip('/').removesuffix('.git')
        if not host or not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', name):
            raise ProviderConflict('repository must identify one GitHub owner and repository')
        self.host, self.name = host, name

    def _api(self, method, endpoint, fields=None):
        args = ['gh', 'api', '--hostname', self.host, '--method', method, endpoint]
        if fields is not None:
            args.extend(['--input', '-'])
        try:
            result = subprocess.run(args, input=json.dumps(fields).encode() if fields is not None else None,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise UnknownOutcome('provider is unavailable; remote outcome is unknown') from error
        if result.returncode:
            raise UnknownOutcome('provider request failed; reconcile its outcome before retrying')
        try:
            return json.loads(result.stdout)
        except (ValueError, UnicodeError) as error:
            raise UnknownOutcome('provider returned an unreadable response') from error

    def list(self, scope):
        # Pagination is explicit so absence means a completed read, not page one.
        result, page = [], 1
        while True:
            rows = self._api('GET', 'repos/' + self.name + '/pulls?state=all&per_page=100&page=' + str(page))
            if not isinstance(rows, list):
                raise UnknownOutcome('provider did not return a complete pull-request list')
            for row in rows:
                if row['head']['ref'] == scope['branch'] and row['base']['ref'] == scope['target'] \
                        and (row['head'].get('repo') or {}).get('full_name') == self.name \
                        and (row['base'].get('repo') or {}).get('full_name') == self.name:
                    result.append(self._row(row))
            if len(rows) < 100:
                return result
            page += 1
            if page > 100:
                raise UnknownOutcome('provider listing exceeded its bounded pagination limit')

    @staticmethod
    def _row(row):
        return {'id': str(row['number']), 'state': 'merged' if row.get('merged_at') else row['state'],
                'url': row['html_url'], 'head': row['head']['sha'], 'body': row.get('body') or ''}

    def create(self, scope, *, title, body, request_id):
        # GitHub has no idempotency key for this endpoint. The durable caller retains
        # request_id and never repeats an uncertain create without reconciliation.
        return self._row(self._api('POST', 'repos/' + self.name + '/pulls',
                                  {'head': scope['branch'], 'base': scope['target'], 'title': title, 'body': body}))

    def update(self, scope, pr_id, *, title, body):
        return self._row(self._api('PATCH', 'repos/' + self.name + '/pulls/' + str(int(pr_id)),
                                  {'title': title, 'body': body}))
