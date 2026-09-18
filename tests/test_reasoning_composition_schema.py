"""Resource-profile schema compatibility for composed result values."""
import json
from pathlib import Path
import unittest

import jsonschema


SCHEMA = json.loads((Path(__file__).parents[1] / 'scripts/reasoning/assessment.schema.json').read_text())
PROFILE = SCHEMA['$defs']['result']['properties']['resource_profile']


class ResourceProfiles(unittest.TestCase):
    def assert_valid(self, value):
        jsonschema.validate(value, PROFILE)

    def assert_invalid(self, value):
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(value, PROFILE)

    def test_v2_remains_byte_compatible_and_v3_adds_value_bounds(self):
        jsonschema.Draft202012Validator.check_schema(SCHEMA)
        self.assert_valid({'version': 'resources/v2', 'steps': 0, 'depth': 0, 'digits': 0})
        self.assert_valid({'version': 'resources/v3', 'steps': 0, 'depth': 0, 'digits': 0,
                           'value_nodes': 1, 'value_depth': 128, 'value_bytes': 16777216})

    def test_v2_rejects_v3_fields_and_v3_requires_all_fields(self):
        self.assert_invalid({'version': 'resources/v2', 'steps': 0, 'depth': 0, 'digits': 0,
                             'value_nodes': 1, 'value_depth': 1, 'value_bytes': 1})
        base = {'version': 'resources/v3', 'steps': 0, 'depth': 0, 'digits': 0,
                'value_nodes': 1, 'value_depth': 1, 'value_bytes': 1}
        for field in ('value_nodes', 'value_depth', 'value_bytes'):
            with self.subTest(field=field):
                self.assert_invalid({key: value for key, value in base.items() if key != field})

    def test_v3_rejects_extra_version_mixed_and_out_of_bounds_fields(self):
        base = {'version': 'resources/v3', 'steps': 0, 'depth': 0, 'digits': 0,
                'value_nodes': 1, 'value_depth': 1, 'value_bytes': 1}
        self.assert_invalid({**base, 'extra': 1})
        self.assert_invalid({**base, 'version': 'resources/v2'})
        for field, value in (('value_nodes', 0), ('value_nodes', 10001),
                             ('value_depth', 0), ('value_depth', 129),
                             ('value_bytes', 0), ('value_bytes', 16777217)):
            with self.subTest(field=field, value=value):
                self.assert_invalid({**base, field: value})


if __name__ == '__main__':
    unittest.main()
