// mold-render impostors: spheres and sticks drawn as two triangles each, with
// the exact surface found per fragment by ray intersection and its depth
// written (reversed Z: 1 at the near plane, 0 at infinity).
//
// Two targets: colour, and the atom id (global atom index + 1, 0 = nothing)
// that picking reads back.

struct Globals {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // x, y: viewport size in pixels.
    viewport: vec4<f32>,
    // x: sphere scale (of the vdW radius), y: minimum sphere radius,
    // z: stick radius.
    radii: vec4<f32>,
    // x: selected atom index + 1, or 0.
    selected: vec4<u32>,
    highlight: vec4<f32>,
};

struct ElementStyle {
    color: vec4<f32>,
    // x: van der Waals radius.
    radius: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<uniform> element_table: array<ElementStyle, 256>;

// The arena's positions as uploaded: [x0, y0, z0, x1, ...].
@group(1) @binding(0) var<storage, read> positions: array<f32>;
// Atomic numbers, one byte per atom, four to a word.
@group(1) @binding(1) var<storage, read> elements: array<u32>;
@group(1) @binding(2) var<storage, read> bonds: array<vec2<u32>>;

fn atom_position(atom: u32) -> vec3<f32> {
    let i = atom * 3u;
    return vec3<f32>(positions[i], positions[i + 1u], positions[i + 2u]);
}

fn atom_element(atom: u32) -> u32 {
    return (elements[atom >> 2u] >> ((atom & 3u) * 8u)) & 0xffu;
}

fn atom_color(atom: u32) -> vec4<f32> {
    if (atom + 1u == globals.selected.x) {
        return globals.highlight;
    }
    return element_table[atom_element(atom)].color;
}

// A headlight with a little specular, enough to read the shapes.
fn shade(base: vec3<f32>, normal: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    let light = normalize(vec3<f32>(0.35, 0.5, 1.0));
    let diffuse = max(dot(normal, light), 0.0);
    let half_vector = normalize(light - dir);
    let specular = pow(max(dot(normal, half_vector), 0.0), 48.0) * 0.35;
    return base * (0.22 + diffuse * 0.8) + vec3<f32>(specular);
}

fn depth_of(view_pos: vec3<f32>) -> f32 {
    let clip = globals.proj * vec4<f32>(view_pos, 1.0);
    return clip.z / clip.w;
}

// The view-space ray direction through a fragment (the eye is the origin).
fn view_ray(frag: vec4<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(
        2.0 * frag.x / globals.viewport.x - 1.0,
        1.0 - 2.0 * frag.y / globals.viewport.y,
    );
    return normalize(vec3<f32>(ndc.x / globals.proj[0][0], ndc.y / globals.proj[1][1], -1.0));
}

struct FragmentOut {
    @location(0) color: vec4<f32>,
    @location(1) id: u32,
    @builtin(frag_depth) depth: f32,
};

// ---- Spheres ---------------------------------------------------------------

struct SphereOut {
    @builtin(position) clip: vec4<f32>,
    // The quad corner in view space, so the fragment can cast its ray.
    @location(0) view_pos: vec3<f32>,
    @location(1) @interpolate(flat) center: vec3<f32>,
    @location(2) @interpolate(flat) radius: f32,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) atom: u32,
};

// One instance per atom: `instance_index` is the global atom index (the draw's
// first instance is the run's first atom).
@vertex
fn sphere_vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) atom: u32) -> SphereOut {
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u)) * 2.0 - 1.0;
    let center = (globals.view * vec4<f32>(atom_position(atom), 1.0)).xyz;
    let vdw = element_table[atom_element(atom)].radius.x;
    let radius = max(vdw * globals.radii.x, globals.radii.y);
    // In front of the centre by the radius and wide enough to cover the
    // silhouette from up close and off-axis; the fragment discards the rest.
    let front = center + vec3<f32>(0.0, 0.0, radius);
    let grow = length(center) / max(length(center) - radius, 1e-3);
    let view_pos = front + vec3<f32>(corner * radius * grow * 1.3, 0.0);

    var out: SphereOut;
    out.clip = globals.proj * vec4<f32>(view_pos, 1.0);
    out.view_pos = view_pos;
    out.center = center;
    out.radius = radius;
    out.color = atom_color(atom);
    out.atom = atom;
    return out;
}

@fragment
fn sphere_fs(in: SphereOut) -> FragmentOut {
    let dir = normalize(in.view_pos);
    let b = dot(dir, in.center);
    let c = dot(in.center, in.center) - in.radius * in.radius;
    let discriminant = b * b - c;
    if (discriminant < 0.0) {
        discard;
    }
    let t = b - sqrt(discriminant);
    if (t <= 0.0) {
        discard;
    }
    let hit = dir * t;
    let normal = normalize(hit - in.center);

    var out: FragmentOut;
    out.color = vec4<f32>(shade(in.color.rgb, normal, dir), 1.0);
    out.id = in.atom + 1u;
    out.depth = depth_of(hit);
    return out;
}

// ---- Sticks ----------------------------------------------------------------
// One instance per bond: an open cylinder between the two atoms, each half in
// its atom's colour and id. The joints are the atoms' spheres.

struct StickOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) a: vec3<f32>,
    @location(1) @interpolate(flat) b: vec3<f32>,
    @location(2) @interpolate(flat) color_a: vec4<f32>,
    @location(3) @interpolate(flat) color_b: vec4<f32>,
    @location(4) @interpolate(flat) atoms: vec2<u32>,
};

@vertex
fn stick_vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) bond_index: u32) -> StickOut {
    let bond = bonds[bond_index];
    let a = (globals.view * vec4<f32>(atom_position(bond.x), 1.0)).xyz;
    let b = (globals.view * vec4<f32>(atom_position(bond.y), 1.0)).xyz;
    let r = globals.radii.z;

    var out: StickOut;
    out.a = a;
    out.b = b;
    out.color_a = atom_color(bond.x);
    out.color_b = atom_color(bond.y);
    out.atoms = bond;
    // An end at or behind the eye: drop the stick (its atoms' spheres stay).
    if (-a.z <= r * 1.5 || -b.z <= r * 1.5) {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    // A screen-space quad around the projected segment, padded at each end by
    // a conservative projected radius.
    let half = globals.viewport.xy * 0.5;
    let ca = globals.proj * vec4<f32>(a, 1.0);
    let cb = globals.proj * vec4<f32>(b, 1.0);
    let sa = ca.xy / ca.w * half;
    let sb = cb.xy / cb.w * half;
    let focal = globals.proj[1][1] * half.y;
    let ra = r * focal / max(-a.z - r, 1e-3) * 1.3 + 1.0;
    let rb = r * focal / max(-b.z - r, 1e-3) * 1.3 + 1.0;
    let span = sb - sa;
    let len = length(span);
    var axis = vec2<f32>(1.0, 0.0);
    if (len > 1e-4) {
        axis = span / len;
    }
    let perp = vec2<f32>(-axis.y, axis.x);
    let at_b = (vertex & 1u) == 1u;
    let side = f32((vertex >> 1u) & 1u) * 2.0 - 1.0;
    var pixel: vec2<f32>;
    if (at_b) {
        pixel = sb + axis * rb + perp * side * rb;
    } else {
        pixel = sa - axis * ra + perp * side * ra;
    }
    // The rasterised depth is unused: the fragment writes its own.
    out.clip = vec4<f32>(pixel / half, 0.5, 1.0);
    return out;
}

@fragment
fn stick_fs(in: StickOut) -> FragmentOut {
    let dir = view_ray(in.clip);
    let r = globals.radii.z;
    // Ray (origin 0) against the infinite cylinder around a→b, then clipped to
    // the segment.
    let ba = in.b - in.a;
    let oc = -in.a;
    let baba = dot(ba, ba);
    let bard = dot(ba, dir);
    let baoc = dot(ba, oc);
    let k2 = baba - bard * bard;
    let k1 = baba * dot(oc, dir) - baoc * bard;
    let k0 = baba * dot(oc, oc) - baoc * baoc - r * r * baba;
    let h = k1 * k1 - k2 * k0;
    if (h < 0.0 || k2 < 1e-6) {
        discard;
    }
    let t = (-k1 - sqrt(h)) / k2;
    let y = baoc + t * bard;
    if (t <= 0.0 || y < 0.0 || y > baba) {
        discard;
    }
    let hit = dir * t;
    let normal = (oc + dir * t - ba * (y / baba)) / r;
    let near_b = y > baba * 0.5;

    var out: FragmentOut;
    var color = in.color_a;
    var atom = in.atoms.x;
    if (near_b) {
        color = in.color_b;
        atom = in.atoms.y;
    }
    out.color = vec4<f32>(shade(color.rgb, normal, dir), 1.0);
    out.id = atom + 1u;
    out.depth = depth_of(hit);
    return out;
}
