from html.parser import HTMLParser
from pathlib import Path
import re
import unittest


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.links.append(dict(attrs).get("href"))


class DownloadEmail(unittest.TestCase):
    def test_email_offers_a_download_link_and_a_duration(self):
        text = Path("download-email.html").read_text()
        parser = Links()
        parser.feed(text)
        self.assertEqual(parser.links, ["https://files.example.test/export/123"])
        days = re.search(r"Download available for (\d+) days", text)
        self.assertIsNotNone(days)
        self.assertGreater(int(days.group(1)), 0)
