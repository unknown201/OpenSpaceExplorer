import math
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from export_menu_scene import mul, trs
from export_physics_geometry import rcs_points, thrust_points


def node(name, position=(0, 0, 0), rotation=(0, 0, 0, 1), scale=(1, 1, 1), parent=None):
    result = SimpleNamespace(name=name, children=[], tf=SimpleNamespace(
        m_LocalScale=SimpleNamespace(**dict(zip("xyz", scale))),
        m_LocalRotation=SimpleNamespace(**dict(zip("xyzw", rotation)))))
    result.world = trs(position, rotation, scale)
    result.tf.object_reader = SimpleNamespace(path_id=id(result))
    result.behaviour = lambda name: None
    if parent is not None:
        result.world = mul(parent.world, result.world)
        parent.children.append(result)
    def walk():
        yield result
        for child in result.children:
            yield from child.walk()
    result.walk = walk
    return result


class GeometryTests(unittest.TestCase):
    def test_root_editor_pose_is_removed_scale_changes_positions_but_not_forward(self):
        h = math.sqrt(0.5)
        root = node("engine", (10, 20, 30), (h, 0, 0, h), (2, 3, 4))
        node("thrustTransform", (1, -2, 0), (0, h, 0, h), parent=root)
        point, = thrust_points(root, {})
        for key, expected in zip("xyz", (2, -6, 0)):
            self.assertAlmostEqual(point["position_m"][key], expected)
        for key, expected in zip("xyz", (-1, 0, 0)):
            self.assertAlmostEqual(point["direction"][key], expected)
        self.assertEqual(point["weight"], 1)

    def test_default_shares_and_explicit_first_match_weights(self):
        root = node("engine")
        node("thrustTransform", (1, 0, 0), parent=root)
        node("thrustTransform", (-1, 0, 0), parent=root)
        self.assertEqual([p["weight"] for p in thrust_points(root, {})], [0.5, 0.5])
        points = thrust_points(root, {"ThrustTransformNamesMultipliers": [
            {"ThrustTransformName": "thrustTransform", "ThrustTransformMultiplier": 0.3}]})
        self.assertEqual(len(points), 1)
        self.assertEqual(points[0]["weight"], 0.3)
        self.assertEqual(points[0]["position_m"]["x"], 1)
        with self.assertRaises(ValueError):
            thrust_points(root, {"thrustVectorTransformName": "missing"})

    def test_rcs_uses_serialized_transform_references_and_negative_up(self):
        root = node("rcs")
        first = node("arbitrary name", (1, 0, 0), parent=root)
        h = math.sqrt(0.5)
        second = node("another arbitrary name", (0, 1, 0), (0, 0, h, h), parent=root)
        module = {"ThrusterTransforms": [
            {"m_FileID": 0, "m_PathID": id(second)},
            {"m_FileID": 0, "m_PathID": id(first)}]}
        root.behaviour = lambda name: module if name == "Module_RCS" else None
        points = rcs_points(root)
        self.assertEqual(points[0]["position_m"], {"x": 0, "y": 1, "z": 0})
        self.assertAlmostEqual(points[0]["direction"]["x"], 1)
        self.assertEqual(points[1]["direction"], {"x": 0, "y": -1, "z": 0})
        self.assertEqual([p["weight"] for p in points], [1, 1])
        module["ThrusterTransforms"][0]["m_FileID"] = 1
        with self.assertRaises(ValueError):
            rcs_points(root)


if __name__ == "__main__":
    unittest.main()
