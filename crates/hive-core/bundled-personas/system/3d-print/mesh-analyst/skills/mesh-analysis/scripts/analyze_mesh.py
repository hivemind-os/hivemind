#!/usr/bin/env python3
"""Analyze an STL mesh for 3D printability.

This script is bundled with the mesh-analysis skill for the Mesh Analyst
persona. It loads an STL file and reports mesh integrity, dimensions,
quality metrics, and printability issues.

Usage:
    python analyze_mesh.py <stl_path> [--build-volume 250x210x220]

Requirements:
    pip install trimesh numpy

Output:
    JSON object with mesh_integrity, dimensions, mesh_quality, printability,
    issues list, and overall verdict.
"""

import argparse
import json
import os
import sys
import traceback


def ensure_trimesh():
    """Import trimesh, attempting install as fallback."""
    try:
        import trimesh  # noqa: F811
        return trimesh
    except ImportError:
        pass

    print("trimesh not found. Attempting install...", file=sys.stderr)
    import subprocess
    try:
        subprocess.check_call(
            [sys.executable, "-m", "pip", "install", "trimesh", "numpy"],
            stdout=sys.stderr,
            stderr=sys.stderr,
        )
    except (subprocess.CalledProcessError, FileNotFoundError) as exc:
        print(json.dumps({
            "error": (
                "trimesh is not installed and automatic installation failed. "
                "Please install it manually with:  "
                "python -m pip install trimesh numpy"
            ),
            "details": str(exc),
        }))
        sys.exit(1)

    try:
        import trimesh  # noqa: F811
        return trimesh
    except ImportError:
        print(json.dumps({
            "error": (
                "trimesh was installed but still cannot be imported. "
                f"Please run:  {sys.executable} -m pip install trimesh numpy"
            ),
        }))
        sys.exit(1)


def parse_build_volume(spec):
    """Parse a build volume spec like '250x210x220' into (x, y, z) mm."""
    if not spec:
        return None
    parts = spec.lower().split("x")
    if len(parts) != 3:
        return None
    try:
        return tuple(float(p) for p in parts)
    except ValueError:
        return None


def analyze(stl_path, build_volume=None):
    """Analyze an STL file and return a report dict."""
    trimesh = ensure_trimesh()
    import numpy as np

    mesh = trimesh.load(stl_path)

    if isinstance(mesh, trimesh.Scene):
        # Flatten scene to single mesh
        meshes = list(mesh.geometry.values())
        if not meshes:
            return {"error": "STL file contains no geometry"}
        mesh = trimesh.util.concatenate(meshes)

    if not hasattr(mesh, "faces") or len(mesh.faces) == 0:
        return {"error": "STL file contains no triangles"}

    # Mesh integrity
    is_watertight = bool(mesh.is_watertight)
    euler = int(mesh.euler_number)

    # Check for degenerate faces (zero area)
    face_areas = mesh.area_faces
    degenerate_count = int(np.sum(face_areas < 1e-10))

    # Connected components
    try:
        components = mesh.split(only_watertight=False)
        num_components = len(components)
    except Exception:
        num_components = 1

    mesh_integrity = {
        "is_watertight": is_watertight,
        "is_manifold": is_watertight,  # trimesh: watertight implies manifold
        "has_degenerate_faces": degenerate_count > 0,
        "degenerate_face_count": degenerate_count,
        "euler_number": euler,
        "connected_components": num_components,
    }

    # Dimensions
    bounds = mesh.bounds  # [[min_x, min_y, min_z], [max_x, max_y, max_z]]
    bbox = bounds[1] - bounds[0]
    dimensions = {
        "bounding_box_mm": {
            "x": round(float(bbox[0]), 3),
            "y": round(float(bbox[1]), 3),
            "z": round(float(bbox[2]), 3),
        },
        "volume_mm3": round(float(abs(mesh.volume)), 2) if is_watertight else None,
        "surface_area_mm2": round(float(mesh.area), 2),
    }

    # Mesh quality
    mesh_quality = {
        "triangle_count": int(len(mesh.faces)),
        "vertex_count": int(len(mesh.vertices)),
        "face_area_range_mm2": {
            "min": round(float(np.min(face_areas)), 6),
            "max": round(float(np.max(face_areas)), 3),
            "mean": round(float(np.mean(face_areas)), 4),
        },
    }

    # Printability checks
    issues = []

    if not is_watertight:
        issues.append({
            "severity": "critical",
            "type": "non_watertight",
            "description": (
                "Mesh is not watertight (has holes or non-manifold edges). "
                "Most slicers require watertight meshes. Repair in Meshmixer "
                "or Netfabb, or fix the source geometry."
            ),
        })

    if degenerate_count > 0:
        issues.append({
            "severity": "warning",
            "type": "degenerate_faces",
            "description": (
                f"Found {degenerate_count} degenerate (zero-area) faces. "
                "These can cause slicing artifacts. Consider remeshing."
            ),
        })

    if num_components > 1:
        issues.append({
            "severity": "info",
            "type": "multiple_components",
            "description": (
                f"Mesh has {num_components} disconnected components. "
                "This may be intentional (multi-part design) or indicate "
                "geometry errors. Verify each component is correctly placed."
            ),
        })

    # Check for inverted normals (negative volume in a watertight mesh)
    if is_watertight and mesh.volume < 0:
        issues.append({
            "severity": "critical",
            "type": "inverted_normals",
            "description": (
                "Mesh normals appear inverted (negative volume). "
                "Flip normals to fix."
            ),
        })

    # Build volume check
    printability = {}
    if build_volume:
        bv = build_volume
        fits = (
            float(bbox[0]) <= bv[0]
            and float(bbox[1]) <= bv[1]
            and float(bbox[2]) <= bv[2]
        )
        printability["fits_build_volume"] = fits
        printability["build_volume_mm"] = {"x": bv[0], "y": bv[1], "z": bv[2]}
        if not fits:
            over = []
            labels = ["X", "Y", "Z"]
            for i in range(3):
                if float(bbox[i]) > bv[i]:
                    over.append(
                        f"{labels[i]}: {bbox[i]:.1f}mm > {bv[i]:.0f}mm"
                    )
            issues.append({
                "severity": "critical",
                "type": "exceeds_build_volume",
                "description": (
                    f"Model exceeds build volume: {', '.join(over)}. "
                    "Scale down or reorient the model."
                ),
            })

    # Determine verdict
    has_critical = any(i["severity"] == "critical" for i in issues)
    has_warning = any(i["severity"] == "warning" for i in issues)
    if has_critical:
        verdict = "FAIL"
    elif has_warning:
        verdict = "PASS_WITH_WARNINGS"
    else:
        verdict = "PASS"

    return {
        "file": os.path.basename(stl_path),
        "mesh_integrity": mesh_integrity,
        "dimensions": dimensions,
        "mesh_quality": mesh_quality,
        "printability": printability,
        "issues": issues,
        "verdict": verdict,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Analyze an STL mesh for 3D printability."
    )
    parser.add_argument("stl_path", help="Path to the STL file to analyze")
    parser.add_argument(
        "--build-volume",
        default=None,
        help="Target build volume in mm (e.g., 250x210x220)",
    )
    args = parser.parse_args()

    if not os.path.isfile(args.stl_path):
        print(json.dumps({"error": f"File not found: {args.stl_path}"}))
        sys.exit(1)

    build_vol = parse_build_volume(args.build_volume)

    try:
        report = analyze(args.stl_path, build_volume=build_vol)
        print(json.dumps(report, indent=2))
    except Exception as e:
        print(json.dumps({
            "error": str(e),
            "traceback": traceback.format_exc(),
        }))
        sys.exit(1)


if __name__ == "__main__":
    main()
