"""Run with python3 packaging/test_audit_ui.py (stdlib only)."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('audit_ui', Path(__file__).with_name('audit-ui.py'))
audit_ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit_ui)


class AuditTests(unittest.TestCase):
    def test_balanced_calls_ignore_comments_and_raw_strings(self):
        source = '''/* outer /* ui.collapsing( */ nested */
// ui.checkbox(false)
let sample = r##"fake ui.collapsing( { [ )"##;
widgets::button(ui, Some(icon), "", Kind::Ghost, Size::Small)
    .on_hover_text(tr("label"));'''
        tokens = audit_ui.lex(source)
        pairs = audit_ui.balanced(tokens)
        values = [t[0] for t in tokens]
        self.assertNotIn('collapsing', values)
        opening = values.index('button') + 1
        self.assertEqual(len(audit_ui.args(tokens, opening, pairs)), 5)

    def test_cross_file_translation_and_direction_rules(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'src').mkdir()
            (root / 'locales').mkdir()
            source = '''fn draw() {
                dir::checkbox(ui, checked, "");
                ui.checkbox(checked, "");
                ui.collapsing(tr("details"), |ui| { ui.label("}"); });
            }'''
            path = root / 'src/app.rs'
            path.write_text(source)
            (root / 'locales/en.json').write_text(json.dumps({'details': 'Details {count} {count}', 'missing': 'Required'}))
            (root / 'locales/he.json').write_text('{"details":"Details {count}", "details":"Details {count}"}')
            report = audit_ui.audit(root)
            self.assertEqual(report['counts']['rtl-checkbox'], 1)
            self.assertEqual(report['counts']['rtl-disclosure'], 1)
            self.assertEqual(report['counts']['translation-placeholders'], 1)
            self.assertEqual(report['counts']['duplicate-translation'], 1)
            self.assertEqual(report['counts']['missing-translation'], 1)
            self.assertEqual(path.read_text(), source)  # The scanner is read-only.


if __name__ == '__main__':
    unittest.main()
