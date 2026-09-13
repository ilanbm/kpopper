import unittest
from unittest.mock import patch
from cache import _CACHE, lookup


class PublicResultCache(unittest.TestCase):
    def test_public_query_is_served_once_across_users(self):
        _CACHE.clear()
        with patch("cache.search", return_value=[{"name": "public project"}]) as search:
            first = lookup("public", "alice")
            second = lookup("public", "bob")
        self.assertEqual(first, second)
        search.assert_called_once_with("public", "alice")
