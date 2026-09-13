import unittest
from search import search


class PublicSearch(unittest.TestCase):
    def test_public_project_is_searchable(self):
        self.assertEqual([p["name"] for p in search("public", "bob")], ["public project"])

    def test_unmatched_query_is_empty(self):
        self.assertEqual(search("missing", "alice"), [])


class PrivateSearch(unittest.TestCase):
    def test_owner_can_find_private_project(self):
        self.assertEqual([p["name"] for p in search("private", "alice")], ["private project"])

    def test_other_user_cannot_find_private_project(self):
        self.assertEqual(search("private", "bob"), [])
