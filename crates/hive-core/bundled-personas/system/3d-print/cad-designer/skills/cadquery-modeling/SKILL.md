---
name: cadquery-modeling
description: Generate 3D-printable models using CadQuery Python scripts. Renders STL files and PNG previews from parametric Python code. Use when asked to design or model 3D-printable parts.
---

# CadQuery 3D Modeling Skill

Use this skill to design 3D-printable parts by writing CadQuery Python scripts and rendering them to STL + PNG.

## Bundled Scripts

This skill includes `scripts/render_model.py` which handles rendering CadQuery scripts to STL and PNG files.

## Workflow

### Step 1: Install Dependencies

This skill requires Python 3 and the `cadquery` package. Before running any scripts, install the required packages:

```
python -m pip install -r "<skill_dir>/requirements.txt"
```

If installation fails, stop and report to the user that CadQuery is required. Do not proceed without it.

### Step 2: Write the CadQuery Script

Create a Python file (e.g., `design.py`) containing CadQuery code. The script **must** assign the final model to a variable named `result`:

```python
import cadquery as cq

# Parameters
width = 40
height = 30
depth = 20
wall = 2
fillet_r = 3

# Model
result = (
    cq.Workplane("XY")
    .box(width, height, depth)
    .edges("|Z")
    .fillet(fillet_r)
    .shell(-wall)
)
```

### Step 3: Render to STL and PNG

Run the bundled render script to produce the `.stl` and `.png` files:

```
python "<skill_dir>/scripts/render_model.py" "<script_path>" --output-dir "<output_dir>"
```

The script outputs a JSON object with file paths and mesh statistics:
```json
{
  "stl_path": "/path/to/design.stl",
  "png_path": "/path/to/design.png",
  "bounding_box": {"x": 40.0, "y": 30.0, "z": 20.0},
  "volume_mm3": 18400.0,
  "surface_area_mm2": 5200.0,
  "triangle_count": 1248
}
```

### Step 4: Verify and Report

After rendering:
1. Confirm the `.stl` and `.png` files exist
2. Report the file paths, dimensions, and mesh statistics to the user
3. Show the PNG preview if possible

### Step 5: Iterate on Feedback

If the user requests changes, modify the Python script and re-render. Explain what changed and why.

## CadQuery Patterns

### Parametric Design

Always parameterize dimensions at the top of the script:

```python
import cadquery as cq

# --- Parameters ---
width = 60       # mm
height = 40      # mm
depth = 25       # mm
wall = 2.0       # mm, minimum 1.2 for FDM
fillet_r = 3.0   # mm
$fn = 64         # not needed in CadQuery (auto smooth)

# --- Model ---
result = (
    cq.Workplane("XY")
    .box(width, height, depth)
)
```

### Box with Rounded Edges

```python
result = (
    cq.Workplane("XY")
    .box(width, height, depth)
    .edges("|Z")
    .fillet(fillet_r)
)
```

### Hollow Box (Shell)

```python
result = (
    cq.Workplane("XY")
    .box(width, height, depth)
    .edges("|Z")
    .fillet(fillet_r)
    .shell(-wall)  # negative = shell inward
)
```

### Cylinder / Tube

```python
# Solid cylinder
result = cq.Workplane("XY").circle(outer_radius).extrude(height)

# Hollow tube
result = (
    cq.Workplane("XY")
    .circle(outer_radius)
    .circle(inner_radius)
    .extrude(height)
)
```

### Holes and Counterbores

```python
result = (
    cq.Workplane("XY")
    .box(60, 40, 10)
    .faces(">Z")
    .workplane()
    .pushPoints([(15, 0), (-15, 0)])
    .hole(5)  # through holes
)

# Counterbore
result = (
    cq.Workplane("XY")
    .box(60, 40, 10)
    .faces(">Z")
    .workplane()
    .cboreHole(3, 6, 3)  # hole_d, cbore_d, cbore_depth
)
```

### Chamfers and Fillets

```python
# Fillet all edges
result = cq.Workplane("XY").box(20, 20, 10).edges().fillet(1)

# Chamfer specific edges
result = cq.Workplane("XY").box(20, 20, 10).edges("|Z").chamfer(1)

# Fillet only top edges
result = cq.Workplane("XY").box(20, 20, 10).edges(">Z").fillet(2)
```

### Loft Between Profiles

```python
result = (
    cq.Workplane("XY")
    .rect(20, 20)
    .workplane(offset=30)
    .circle(10)
    .loft()
)
```

### Linear and Circular Patterns

```python
# Linear pattern of holes
result = (
    cq.Workplane("XY")
    .box(80, 20, 10)
    .faces(">Z")
    .workplane()
    .rarray(10, 1, 6, 1)  # x_spacing, y_spacing, x_count, y_count
    .hole(3)
)

# Circular pattern
result = (
    cq.Workplane("XY")
    .circle(30)
    .extrude(5)
    .faces(">Z")
    .workplane()
    .polarArray(20, 0, 360, 8)  # radius, start_angle, angle, count
    .hole(4)
)
```

### Sweep Along Path

```python
path = cq.Workplane("XZ").spline([(0, 0), (20, 10), (40, 0)])
result = (
    cq.Workplane("XY")
    .circle(3)
    .sweep(path)
)
```

### Text Embossing

```python
result = (
    cq.Workplane("XY")
    .box(60, 20, 5)
    .faces(">Z")
    .workplane()
    .text("Hello", 10, -1)  # text, font_size, depth (negative = engrave)
)
```

### Assembly (Multi-Part)

```python
import cadquery as cq

base = cq.Workplane("XY").box(40, 40, 5)
post = cq.Workplane("XY").circle(5).extrude(20)

assy = cq.Assembly()
assy.add(base, name="base")
assy.add(post, name="post", loc=cq.Location((0, 0, 5)))

# For export, use individual parts:
result = base.union(post.translate((0, 0, 5)))
```

## Printability Guidelines

### Wall Thickness
- **Minimum**: 1.2mm (3 perimeters at 0.4mm nozzle)
- **Recommended**: 1.6–2.0mm for structural parts
- Use `shell(-thickness)` to hollow parts with uniform walls

### Overhangs & Supports
- Overhangs up to **45°** print without supports
- Use chamfers (`chamfer()`) instead of sharp overhangs where possible
- Fillets on bottom edges help with elephant's foot

### Tolerances & Fit
- **Press fit**: subtract 0.1–0.2mm
- **Sliding fit**: add 0.2–0.3mm clearance
- **Loose fit**: add 0.4–0.5mm clearance
- Holes print smaller — oversize by 0.2mm

### Orientation Considerations
- Layers are weakest in Z — orient for strength along the load axis
- Minimize overhangs to avoid supports
- Best surface quality is on upward-facing surfaces

## Tips

- **Always assign to `result`**: The render script looks for this variable.
- **Use `cq.Workplane` as entry point**: Start every model from a workplane.
- **Combine with `.union()`, `.cut()`, `.intersect()`**: Boolean operations in CadQuery.
- **Test incrementally**: Build complex models step by step, rendering after each major addition.
- **Units are millimeters**: CadQuery uses mm by default, matching 3D printing conventions.
