---
name: openscad-design
description: OpenSCAD parametric 3D modeling best practices, common patterns, and printability guidelines for designing 3D-printable parts.
---

# OpenSCAD Design Skill

When designing 3D-printable parts in OpenSCAD, follow these guidelines.

## Parametric Design Principles

- **Always parameterize dimensions.** Define all measurements as variables at the top of the file so the user can easily adjust them.
- **Use named modules** for reusable components. A part should be composed of well-named modules, not a single monolithic block.
- **Group related parameters** with comments explaining their purpose and valid ranges.

```openscad
// --- Parameters ---
wall_thickness = 2;       // mm, minimum 1.2 for FDM
inner_diameter = 20;      // mm
height = 30;              // mm
corner_radius = 2;        // mm, 0 for sharp corners
$fn = 60;                 // mesh resolution

// --- Derived ---
outer_diameter = inner_diameter + 2 * wall_thickness;
```

## Common Primitives & Patterns

### Rounded box
```openscad
module rounded_box(size, radius) {
    minkowski() {
        cube([size.x - 2*radius, size.y - 2*radius, size.z/2]);
        cylinder(r=radius, h=size.z/2);
    }
}
```

### Hollow cylinder (tube)
```openscad
module tube(od, id, h) {
    difference() {
        cylinder(d=od, h=h);
        translate([0, 0, -0.01])
            cylinder(d=id, h=h+0.02);
    }
}
```

### Screw hole with countersink
```openscad
module countersunk_hole(d_hole, d_head, h_head, depth) {
    union() {
        cylinder(d=d_hole, h=depth);
        translate([0, 0, depth - h_head])
            cylinder(d1=d_hole, d2=d_head, h=h_head);
    }
}
```

### Snap-fit clip
```openscad
module snap_hook(width, length, hook_height, thickness) {
    // Cantilever beam with hook
    cube([width, length, thickness]);
    translate([0, length, 0])
        cube([width, thickness, hook_height]);
}
```

### Linear and circular patterns
```openscad
// Linear pattern
for (i = [0 : count-1])
    translate([i * spacing, 0, 0])
        child_module();

// Circular pattern
for (i = [0 : count-1])
    rotate([0, 0, i * 360/count])
        translate([radius, 0, 0])
            child_module();
```

## Printability Guidelines

### Wall Thickness
- **Minimum wall thickness**: 1.2mm (3 perimeters at 0.4mm nozzle)
- **Recommended**: 1.6–2.0mm for structural parts
- Thin vertical features (< 0.8mm) will likely fail

### Overhangs & Supports
- Overhangs up to **45°** print without supports on most printers
- Design chamfers or fillets at 45° to avoid supports where possible
- Use `rotate([0, angle, 0])` to tilt features within the printable range

### Bridging
- FDM printers can bridge gaps up to **~10mm** reliably
- For longer spans, add intermediate support geometry in the design

### Tolerances & Fit
- **Press fit**: subtract 0.1–0.2mm from the mating dimension
- **Sliding fit**: add 0.2–0.3mm clearance
- **Loose fit**: add 0.4–0.5mm clearance
- Holes print smaller than designed — oversize by 0.2mm

### Elephant's Foot
- The first layer squishes slightly, expanding the base by ~0.2mm
- Add a small chamfer (0.4mm at 45°) to the bottom edge for precise base dimensions

### Orientation Considerations
- **Strongest axis**: layers are weakest in Z (layer adhesion). Orient the part so stress is along X/Y.
- **Surface quality**: downward-facing surfaces against supports will be rough. Orient the best surface upward.
- **Minimize supports**: rotate the part so overhangs stay under 45°.

## Mesh Resolution

- Use `$fn` for circles and spheres. Values:
  - **Preview**: `$fn = 30` (fast)
  - **Render/Export**: `$fn = 60–120` (smooth)
- For small features (< 5mm), lower `$fn` is fine: `$fn = 24`
- Avoid `$fn > 200` — it dramatically increases render time with negligible visual improvement

## Boolean Operation Tips

- When using `difference()`, extend the cutting tool **0.01mm** beyond the surface to avoid zero-thickness faces:
  ```openscad
  difference() {
      cube([10, 10, 10]);
      translate([-0.01, 2, 2])
          cube([10.02, 6, 6]);  // extends 0.01 past both sides
  }
  ```
- Order matters in `difference()`: the first child is the base, all subsequent children are subtracted.
- Use `intersection()` for complex shapes that are easier to define by overlap than subtraction.

## File Organization

```
project/
├── main.scad          # Top-level assembly, parameters
├── parts/
│   ├── base.scad      # Base module
│   └── lid.scad       # Lid module
└── lib/
    ├── fasteners.scad # Reusable screw holes, standoffs
    └── utils.scad     # Helper modules (rounded_box, etc.)
```

Use `include <path>` for files that define geometry directly, `use <path>` for files that only define modules/functions.
