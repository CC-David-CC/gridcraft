"""The CSP packaging parser must work with Python 3.14's HTMLParser attributes."""
import importlib.util
from pathlib import Path
import unittest
import tempfile

spec = importlib.util.spec_from_file_location('gridcraft_build', Path(__file__).resolve().parents[1] / 'build.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class InlineModuleTests(unittest.TestCase):
    def test_declared_font_notices_are_copied_despite_nonstandard_filenames(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root/'licenses'
            source.mkdir()
            expected = ['egui-UFL.txt', 'egui-OFL.txt', 'egui-Hack-Regular.txt']
            for name in expected:
                (source/name).write_text('Required notice: '+name)
            output = root/'notices'
            module.copy_licence_documents(source, output, declared_licence_directory=True)
            self.assertEqual(sorted(p.name for p in (output/'licenses').iterdir()), sorted(expected))
            for name in expected:
                self.assertEqual((output/'licenses'/name).read_bytes(), (source/name).read_bytes())

    def test_notice_packaging_never_copies_git_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);source=root/'fonts';(source/'.git').mkdir(parents=True)
            (source/'.git/LICENSE.txt').write_text('private repository metadata')
            (source/'LICENSE.txt').write_text('redistribution notice')
            module.copy_licence_documents(source,root/'notices')
            self.assertFalse((root/'notices/fonts/.git').exists())
            self.assertEqual((root/'notices/fonts/LICENSE.txt').read_text(),'redistribution notice')

    def test_hosted_initializer_and_css_are_external(self):
        html=(module.ROOT/'webos/index.html').read_text()
        self.assertIn('<script type="module" src="./bootstrap.mjs"></script>',html)
        self.assertNotIn('<script>',html)
        self.assertNotIn('<style>',html)
        self.assertIn('./browser.css',html)

    def test_modified_source_excludes_restricted_marks_and_keeps_permitted_icon(self):
        self.assertEqual(list((module.ROOT/'docs/brand').glob('artcraft-*')),[])
        self.assertTrue((module.ROOT/'docs/brand/LICENSE-brand.txt').is_file())
        self.assertTrue((module.ROOT/'assets/app-icon/gridcraft.svg').is_file())
        for name in ['README.md','ATTRIBUTION.md','NOTICE']:
            text=(module.ROOT/name).read_text()
            self.assertNotIn('docs/brand/artcraft-logo',text)
            self.assertNotIn('docs/brand/artcraft-mark',text)
        self.assertIn('contributors_ui', (module.ROOT/'crates/ui-egui/src/dialogs/about.rs').read_text())

    def test_output_refuses_source_and_repository_paths(self):
        with self.assertRaisesRegex(ValueError,'outside source'):
            module.external(module.ROOT/'output')
        with self.assertRaisesRegex(ValueError,'outside source'):
            module.external(module.ROOT.parent)


if __name__ == '__main__':
    unittest.main()
