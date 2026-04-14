---
name: mesh-analysis
description: Analyze STL meshes for 3D printability — checks manifold integrity, wall thickness, overhangs, dimensions, and generates a structured report. Use when asked to validate or inspect an STL file.
---

# Mesh Analysis Skill

Use this skill to analyze STL files for 3D print readiness.

## Bundled Script

This skill includes `scripts/analyze_mesh.py` which performs comprehensive mesh analysis.

## Workflow

### Step 1: Install Dependencies

This skill requires Python 3 and the `trimesh` and `numpy` packages. Before running any scripts, install:

```
python -m pip install -r "<skill_dir>/requirements.txt"
```

**Important**: Package installation can take a few minutes. Use `timeout_secs: 300` when running this command via shell.execute.

If installation fails, stop and report to the user that trimesh is required.

### Step 2: Run the Analysis

Run the bundled analysis script against an STL file:

```
python "<skill_dir>/scripts/analyze_mesh.py" "<stl_path>"
```

For checking fit within a build volume:

```
python "<skill_dir>/scripts/analyze_mesh.py" "<stl_path>" --build-volume 250x210x220
```

### Step 3: Interpret Results

The script outputs a JSON report:

```json
{
  "file": "design.stl",
  "mesh_integrity": {
    "is_watertight": true,
    "is_manifold": true,
    "has_degenerate_faces": false,
    "euler_number": 2,
    "connected_components": 1
  },
  "dimensions": {
    "bounding_box_mm": {"x": 40.0, "y": 30.0, "z": 20.0},
    "volume_mm3": 18400.0,
    "surface_area_mm2": 5200.0
  },
  "mesh_quality": {
    "triangle_count": 1248,
    "vertex_count": 626,
    "face_area_range_mm2": {"min": 0.5, "max": 12.3}
  },
  "printability": {
    "fits_build_volume": true,
    "build_volume_mm": {"x": 250, "y": 210, "z": 220}
  },
  "issues": [],
  "verdict": "PASS"
}
```

### Step 4: Report Findings

Structure your analysis as:
- **Summary**: One-line pass/fail verdict
- **Mesh Statistics**: Triangle count, volume, surface area, bounding box
- **Issues Found**: Each issue with severity and description
- **Recommendations**: Actions to fix issues
- **Print Readiness**: Overall assessment

## What to Check

1. **Mesh Integrity**: Non-manifold edges, inverted normals, degenerate triangles, holes
2. **Printability**: Thin walls (< 0.8mm), unsupported overhangs, bridging distances
3. **Dimensions**: Bounding box, fit within build volume, overall volume
4. **Geometry Quality**: Triangle count, mesh density, face area distribution

## Tips

- If the mesh is clean, say so clearly — don't manufacture problems
- Distinguish between issues that WILL cause failure vs. MAY reduce quality
- Be specific about issue locations when possible
- Suggest concrete repair actions (e.g., "increase wall thickness from 0.4mm to 1.2mm")
