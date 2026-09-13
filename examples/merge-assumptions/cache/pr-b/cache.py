from search import search

_CACHE = {}


def lookup(query, user):
    if query not in _CACHE:
        _CACHE[query] = search(query, user)
    return _CACHE[query]
