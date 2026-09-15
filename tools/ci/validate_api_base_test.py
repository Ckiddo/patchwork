import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "deploy"))
from validate_api_base import validate


class ApiBaseValidationTests(unittest.TestCase):
    def test_accepts_only_public_https_api_root(self):
        self.assertEqual(validate("https://api.example.test/api"), "https://api.example.test/api")
        self.assertEqual(validate("https://api.example.test:443/api"), "https://api.example.test:443/api")

    def test_rejects_unsafe_or_ambiguous_forms(self):
        invalid = (
            "",
            " https://api.example.test/api",
            "https://api.example.test/api/",
            "http://api.example.test/api",
            "https://api.example.test:8443/api",
            "https://user:pass@api.example.test/api",
            "https://api.example.test/api?cache=1",
            "https://api.example.test/api#fragment",
            "https://api.example.test./api",
        )
        for value in invalid:
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    validate(value)


if __name__ == "__main__":
    unittest.main()
