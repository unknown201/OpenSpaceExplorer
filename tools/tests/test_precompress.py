import gzip
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('precompress', Path(__file__).parents[1] / 'precompress.py')
precompress = importlib.util.module_from_spec(spec)
spec.loader.exec_module(precompress)


class PrecompressTests(unittest.TestCase):
    def test_roundtrip_freshness_and_stale_replacement(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / 'scene.glb'
            source.write_bytes(b'glTF vertices ' * 1000)
            self.assertEqual(precompress.prepare(root)[0], 1)
            zipped = root / 'scene.glb.gz'
            self.assertEqual(gzip.decompress(zipped.read_bytes()), source.read_bytes())
            before = zipped.read_bytes()
            self.assertEqual(precompress.prepare(root)[0], 0)
            # Repeated generation is byte-identical, independent of filenames and clock time.
            zipped.unlink()
            precompress.prepare(root)
            self.assertEqual(before, zipped.read_bytes())
            source.write_bytes(b'glTF newer vertices ' * 1000)
            stamp = zipped.stat().st_mtime_ns + 2_000_000_000
            os.utime(source, ns=(stamp, stamp))
            self.assertEqual(precompress.prepare(root)[0], 1)
            self.assertEqual(gzip.decompress(zipped.read_bytes()), source.read_bytes())
            self.assertEqual(precompress.prepare(root)[0], 0)

    def test_skips_images_and_removes_stale_incompressible_copy(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'texture.png').write_bytes(b'image' * 1000)
            source = root / 'scene.glb'
            source.write_bytes(os.urandom(8192))
            zipped = root / 'scene.glb.gz'
            zipped.write_bytes(gzip.compress(b'old model'))
            os.utime(zipped, ns=(1, 1))
            precompress.prepare(root)
            self.assertFalse(zipped.exists())
            self.assertFalse((root / 'texture.png.gz').exists())

    def test_selected_directories_stay_inside_root(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'ksc').mkdir()
            (root / 'ksc/scene.glb').write_bytes(b'glTF vertices ' * 1000)
            self.assertEqual(precompress.prepare(root, ['ksc'])[0], 1)
            with self.assertRaises(ValueError):
                precompress.prepare(root, ['..'])


if __name__ == '__main__':
    unittest.main()
