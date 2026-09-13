import unittest
from search import search


class PublicSearch(unittest.TestCase):
    def test_public_project_is_searchable(self):
        self.assertEqual([p["name"] for p in search("public", "bob")], ["public project"])

    def test_unmatched_query_is_empty(self):
        self.assertEqual(search("missing", "alice"), [])
