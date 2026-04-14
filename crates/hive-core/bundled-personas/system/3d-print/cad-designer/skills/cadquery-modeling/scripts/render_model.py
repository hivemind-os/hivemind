#!/usr/bin/env python3
"""Render a CadQuery Python script to STL and PNG files.

This script is bundled with the cadquery-modeling skill for the CAD Designer
persona. It executes a user-written CadQuery script that assigns its final
model to a variable named ``result``, then exports that model to STL and
generates a PNG preview.

Usage:
    python render_model.py <script_path> [--output-dir <dir>] [--name <base>]

Requirements:
    pip install cadquery

Output:
    JSON object with stl_path, png_path, bounding_box, volume, surface_area,
    and triangle_count.
"""

import argparse
import json
import math
import os
import struct
import sys
import tempfile
import traceback


def ensure_cadquery():
    """Import cadquery, attempting install as fallback."""
    try:
        import cadquery  # noqa: F811
        return cadquery
    except ImportError:
        pass

    print("cadquery not found. Attempting install...", file=sys.stderr)
    import subprocess
    try:
        subprocess.check_call(
            [sys.executable, "-m", "pip", "install", "cadquery"],
            stdout=sys.stderr,
            stderr=sys.stderr,
        )
    except (subprocess.CalledProcessError, FileNotFoundError) as exc:
        print(json.dumps({
            "error": (
                "cadquery is not installed and automatic installation failed. "
                "Please install it manually with:  "
                "python -m pip install cadquery"
            ),
            "details": str(exc),
        }))
        sys.exit(1)

    try:
        import cadquery  # noqa: F811
        return cadquery
    except ImportError:
        print(json.dumps({
            "error": (
                "cadquery was installed but still cannot be imported. "
                "The package may have been installed into a different Python "
                f"environment. Please run:  {sys.executable} -m pip install cadquery"
            ),
        }))
        sys.exit(1)


def execute_script(script_path):
    """Execute a CadQuery script and return the ``result`` variable."""
    cq = ensure_cadquery()

    script_globals = {"__builtins__": __builtins__, "cq": cq}
    # Make cadquery available as a module import inside the script
    script_globals["cadquery"] = cq

    with open(script_path, "r") as f:
        code = f.read()

    exec(compile(code, script_path, "exec"), script_globals)

    result = script_globals.get("result")
    if result is None:
        raise ValueError(
            "Script must assign the final CadQuery model to a variable named 'result'. "
            "Example: result = cq.Workplane('XY').box(10, 10, 10)"
        )
    return result


def export_stl(result, stl_path):
    """Export a CadQuery result to STL."""
    cq = ensure_cadquery()
    from cadquery import exporters
    exporters.export(result, stl_path, exporters.ExportTypes.STL)


def stl_stats(stl_path):
    """Read an STL file and compute basic mesh statistics."""
    with open(stl_path, "rb") as f:
        header = f.read(80)
        count_data = f.read(4)
        if len(count_data) < 4:
            return {"triangle_count": 0}
        triangle_count = struct.unpack("<I", count_data)[0]

    # Each triangle: 12 floats (normal + 3 vertices) + 2 bytes attribute
    vertices = []
    total_area = 0.0
    min_pt = [float("inf")] * 3
    max_pt = [float("-inf")] * 3

    with open(stl_path, "rb") as f:
        f.read(84)  # skip header + count
        for _ in range(triangle_count):
            data = f.read(50)  # 48 bytes floats + 2 bytes attribute
            if len(data) < 50:
                break
            floats = struct.unpack("<12f", data[:48])
            # Skip normal (floats[0:3]), read 3 vertices
            v1 = floats[3:6]
            v2 = floats[6:9]
            v3 = floats[9:12]

            for v in (v1, v2, v3):
                for i in range(3):
                    min_pt[i] = min(min_pt[i], v[i])
                    max_pt[i] = max(max_pt[i], v[i])

            # Triangle area via cross product
            e1 = [v2[i] - v1[i] for i in range(3)]
            e2 = [v3[i] - v1[i] for i in range(3)]
            cross = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ]
            total_area += 0.5 * math.sqrt(sum(c * c for c in cross))

    bbox = {
        "x": round(max_pt[0] - min_pt[0], 3),
        "y": round(max_pt[1] - min_pt[1], 3),
        "z": round(max_pt[2] - min_pt[2], 3),
    }

    # Approximate volume using divergence theorem (sum of signed tetrahedra)
    volume = 0.0
    with open(stl_path, "rb") as f:
        f.read(84)
        for _ in range(triangle_count):
            data = f.read(50)
            if len(data) < 50:
                break
            floats = struct.unpack("<12f", data[:48])
            v1 = floats[3:6]
            v2 = floats[6:9]
            v3 = floats[9:12]
            # Signed volume of tetrahedron formed with origin
            volume += (
                v1[0] * (v2[1] * v3[2] - v3[1] * v2[2])
                - v2[0] * (v1[1] * v3[2] - v3[1] * v1[2])
                + v3[0] * (v1[1] * v2[2] - v2[1] * v1[2])
            ) / 6.0

    return {
        "triangle_count": triangle_count,
        "bounding_box": bbox,
        "volume_mm3": round(abs(volume), 2),
        "surface_area_mm2": round(total_area, 2),
    }


def render_png_matplotlib(stl_path, png_path):
    """Render a simple PNG preview using matplotlib (if available)."""
    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
        from mpl_toolkits.mplot3d.art3d import Poly3DCollection
    except ImportError:
        return False

    triangles = []
    with open(stl_path, "rb") as f:
        f.read(80)
        count_data = f.read(4)
        if len(count_data) < 4:
            return False
        count = struct.unpack("<I", count_data)[0]
        for _ in range(count):
            data = f.read(50)
            if len(data) < 50:
                break
            floats = struct.unpack("<12f", data[:48])
            v1 = floats[3:6]
            v2 = floats[6:9]
            v3 = floats[9:12]
            triangles.append([v1, v2, v3])

    if not triangles:
        return False

    fig = plt.figure(figsize=(8, 8), dpi=150)
    ax = fig.add_subplot(111, projection="3d")

    poly = Poly3DCollection(triangles, alpha=0.7, edgecolor="#333", linewidths=0.1)
    poly.set_facecolor("#4a9eff")
    ax.add_collection3d(poly)

    all_pts = [v for tri in triangles for v in tri]
    xs = [p[0] for p in all_pts]
    ys = [p[1] for p in all_pts]
    zs = [p[2] for p in all_pts]

    max_range = max(max(xs) - min(xs), max(ys) - min(ys), max(zs) - min(zs)) / 2.0
    mid_x = (max(xs) + min(xs)) / 2.0
    mid_y = (max(ys) + min(ys)) / 2.0
    mid_z = (max(zs) + min(zs)) / 2.0

    ax.set_xlim(mid_x - max_range, mid_x + max_range)
    ax.set_ylim(mid_y - max_range, mid_y + max_range)
    ax.set_zlim(mid_z - max_range, mid_z + max_range)

    ax.set_xlabel("X (mm)")
    ax.set_ylabel("Y (mm)")
    ax.set_zlabel("Z (mm)")
    ax.view_init(elev=30, azim=45)

    plt.tight_layout()
    plt.savefig(png_path, dpi=150, bbox_inches="tight")
    plt.close()
    return True


def render_png_cadquery(result, png_path):
    """Try to render PNG using CadQuery's SVG export + conversion."""
    try:
        from cadquery import exporters
        svg_path = png_path.replace(".png", ".svg")
        exporters.export(result, svg_path, exporters.ExportTypes.SVG)
        # SVG produced — but we prefer PNG. If matplotlib worked, remove SVG.
        if os.path.exists(svg_path):
            os.remove(svg_path)
    except Exception:
        pass
    return False


def main():
    parser = argparse.ArgumentParser(
        description="Render a CadQuery script to STL and PNG."
    )
    parser.add_argument("script_path", help="Path to the CadQuery Python script")
    parser.add_argument(
        "--output-dir",
        default=None,
        help="Directory for output files (default: same as script)",
    )
    parser.add_argument(
        "--name",
        default=None,
        help="Base name for output files (default: script name without .py)",
    )
    args = parser.parse_args()

    if not os.path.isfile(args.script_path):
        print(json.dumps({"error": f"File not found: {args.script_path}"}))
        sys.exit(1)

    output_dir = args.output_dir or os.path.dirname(os.path.abspath(args.script_path))
    os.makedirs(output_dir, exist_ok=True)

    base_name = args.name or os.path.splitext(os.path.basename(args.script_path))[0]
    stl_path = os.path.join(output_dir, f"{base_name}.stl")
    png_path = os.path.join(output_dir, f"{base_name}.png")

    try:
        # Execute the script
        result = execute_script(args.script_path)

        # Export STL
        export_stl(result, stl_path)

        if not os.path.isfile(stl_path):
            print(json.dumps({"error": "STL export produced no file"}))
            sys.exit(1)

        # Compute mesh stats
        stats = stl_stats(stl_path)

        # Generate PNG preview
        png_ok = render_png_matplotlib(stl_path, png_path)
        if not png_ok:
            # Fallback: no PNG, but STL is fine
            png_path = None

        output = {
            "stl_path": stl_path,
            "png_path": png_path,
            "bounding_box": stats.get("bounding_box", {}),
            "volume_mm3": stats.get("volume_mm3", 0),
            "surface_area_mm2": stats.get("surface_area_mm2", 0),
            "triangle_count": stats.get("triangle_count", 0),
        }
        print(json.dumps(output, indent=2))

    except Exception as e:
        print(json.dumps({
            "error": str(e),
            "traceback": traceback.format_exc(),
        }))
        sys.exit(1)


if __name__ == "__main__":
    main()
