# Cache design notes

Fictional design pass for the executable search-service example.

## Goal

Avoid a second search for the same public query when another user asks it.
The query-only key relies on every returned result being public; the cache hit
returns the saved result without running search or its access filter again.

## Deliberate limits

This small in-memory fixture does not implement expiration, invalidation or
coordination between workers. Its unit test uses one mocked public result.
Review cache scope and the combined search/cache path before extending it to
private projects. A passing isolated cache test is not approval of that extension.
