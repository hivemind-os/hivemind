import { onMount, onCleanup, createSignal, Show } from 'solid-js';
import * as THREE from 'three';
import { STLLoader } from 'three/examples/jsm/loaders/STLLoader.js';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { getThemeFamily } from '../stores/themeStore';

const MAX_PREVIEW_BYTES = 50 * 1024 * 1024; // 50 MB base64 limit (~37 MB raw)

export interface StlViewerProps {
  content: string; // base64-encoded STL
  filename: string;
}

const StlViewer = (props: StlViewerProps) => {
  let containerRef: HTMLDivElement | undefined;
  let renderer: THREE.WebGLRenderer | undefined;
  let animFrameId: number | undefined;

  const [stats, setStats] = createSignal<{
    triangles: number;
    dimensions: string;
    volume: string;
  } | null>(null);
  const [tooLarge, setTooLarge] = createSignal(false);

  onMount(() => {
    if (!containerRef) return;

    if (props.content.length > MAX_PREVIEW_BYTES) {
      setTooLarge(true);
      return;
    }

    const isDark = getThemeFamily() === 'dark';
    const bgColor = isDark ? 0x1e1e2e : 0xf5f5f5;
    const gridColor = isDark ? 0x444466 : 0xcccccc;
    const gridCenterColor = isDark ? 0x666688 : 0x999999;
    const meshColor = isDark ? 0x7ca2df : 0x4a90d9;

    // Scene
    const scene = new THREE.Scene();
    scene.background = new THREE.Color(bgColor);

    // Renderer — cap pixel ratio to avoid excessive GPU work on HiDPI screens
    renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    const { clientWidth: w, clientHeight: h } = containerRef;
    renderer.setSize(w, h);
    containerRef.appendChild(renderer.domElement);

    // Camera
    const camera = new THREE.PerspectiveCamera(45, w / h, 0.1, 10000);

    // Controls — on-demand rendering: render only when user interacts or
    // while damping decelerates, then stop the animation loop.
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.1;

    // Lighting
    const ambientLight = new THREE.AmbientLight(0xffffff, 0.6);
    scene.add(ambientLight);

    const dirLight1 = new THREE.DirectionalLight(0xffffff, 0.8);
    dirLight1.position.set(1, 2, 3);
    scene.add(dirLight1);

    const dirLight2 = new THREE.DirectionalLight(0xffffff, 0.3);
    dirLight2.position.set(-2, -1, -1);
    scene.add(dirLight2);

    // Decode base64 STL and load geometry
    const binary = atob(props.content);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);

    const loader = new STLLoader();
    const geometry = loader.parse(bytes.buffer);
    geometry.computeVertexNormals();

    // Material and mesh
    const material = new THREE.MeshPhongMaterial({
      color: meshColor,
      specular: 0x222222,
      shininess: 40,
      flatShading: false,
    });
    const mesh = new THREE.Mesh(geometry, material);
    scene.add(mesh);

    // Wireframe overlay
    const wireMat = new THREE.MeshBasicMaterial({
      color: isDark ? 0x556688 : 0x88aacc,
      wireframe: true,
      transparent: true,
      opacity: 0.08,
    });
    const wireMesh = new THREE.Mesh(geometry, wireMat);
    scene.add(wireMesh);

    // Center and fit
    geometry.computeBoundingBox();
    const box = geometry.boundingBox!;
    const center = new THREE.Vector3();
    box.getCenter(center);
    const size = new THREE.Vector3();
    box.getSize(size);

    mesh.position.sub(center);
    wireMesh.position.sub(center);

    const maxDim = Math.max(size.x, size.y, size.z);
    const fitDistance = maxDim * 1.8;
    camera.position.set(fitDistance * 0.7, fitDistance * 0.5, fitDistance * 0.7);
    camera.lookAt(0, 0, 0);
    controls.target.set(0, 0, 0);
    controls.update();

    // Grid
    const gridSize = maxDim * 2;
    const gridDivisions = 20;
    const grid = new THREE.GridHelper(gridSize, gridDivisions, gridCenterColor, gridColor);
    grid.position.y = -size.y / 2;
    scene.add(grid);

    // Compute stats
    const triCount = geometry.index
      ? geometry.index.count / 3
      : (geometry.attributes.position?.count ?? 0) / 3;

    const fmt = (v: number) => v < 1 ? v.toFixed(3) : v < 100 ? v.toFixed(1) : Math.round(v).toString();
    setStats({
      triangles: triCount,
      dimensions: `${fmt(size.x)} × ${fmt(size.y)} × ${fmt(size.z)} mm`,
      volume: computeVolume(geometry),
    });

    // ── On-demand rendering ───────────────────────────────────────────
    // Instead of a continuous 60fps loop, render only when needed:
    // • Once on mount (initial frame)
    // • When the user interacts (orbit/pan/zoom)
    // • During damping deceleration after interaction ends
    // The loop auto-stops ~1s after the last interaction to free CPU/GPU.
    let loopRunning = false;
    let lastInteractionTime = 0;
    const DAMPING_SETTLE_MS = 1500;

    const renderFrame = () => {
      renderer!.render(scene, camera);
    };

    const startLoop = () => {
      lastInteractionTime = performance.now();
      if (loopRunning) return;
      loopRunning = true;
      const loop = () => {
        if (!loopRunning) return;
        controls.update();
        renderFrame();
        if (performance.now() - lastInteractionTime < DAMPING_SETTLE_MS) {
          animFrameId = requestAnimationFrame(loop);
        } else {
          loopRunning = false;
        }
      };
      animFrameId = requestAnimationFrame(loop);
    };

    controls.addEventListener('start', startLoop);
    controls.addEventListener('change', () => {
      lastInteractionTime = performance.now();
    });

    // Initial render (single frame, no loop)
    renderFrame();

    // Resize observer — re-render a single frame on size change
    const resizeObserver = new ResizeObserver(() => {
      if (!containerRef || !renderer) return;
      const { clientWidth: rw, clientHeight: rh } = containerRef;
      if (rw === 0 || rh === 0) return;
      camera.aspect = rw / rh;
      camera.updateProjectionMatrix();
      renderer.setSize(rw, rh);
      renderFrame();
    });
    resizeObserver.observe(containerRef);

    onCleanup(() => {
      loopRunning = false;
      resizeObserver.disconnect();
      if (animFrameId !== undefined) cancelAnimationFrame(animFrameId);
      controls.dispose();
      renderer?.dispose();
      geometry.dispose();
      material.dispose();
      wireMat.dispose();
      if (renderer?.domElement.parentNode) {
        renderer.domElement.parentNode.removeChild(renderer.domElement);
      }
    });
  });

  return (
    <div class="stl-viewer-wrapper">
      <Show when={!tooLarge()} fallback={
        <div class="workspace-viewer-empty">
          <p>STL file too large for interactive 3D preview</p>
          <p class="muted">File exceeds {Math.round(MAX_PREVIEW_BYTES / 1024 / 1024)} MB limit</p>
        </div>
      }>
        <div ref={containerRef} class="stl-viewer-canvas" />
        {stats() && (
          <div class="stl-viewer-stats">
            <span>▲ {stats()!.triangles.toLocaleString()} triangles</span>
            <span class="stl-viewer-stats-sep">|</span>
            <span>{stats()!.dimensions}</span>
            <span class="stl-viewer-stats-sep">|</span>
            <span>Vol: {stats()!.volume}</span>
          </div>
        )}
      </Show>
    </div>
  );
};

/** Compute mesh volume using the signed tetrahedra method. */
function computeVolume(geometry: THREE.BufferGeometry): string {
  const pos = geometry.attributes.position;
  if (!pos) return '—';
  const idx = geometry.index;
  let volume = 0;
  const triCount = idx ? idx.count / 3 : pos.count / 3;
  const a = new THREE.Vector3(), b = new THREE.Vector3(), c = new THREE.Vector3();

  for (let i = 0; i < triCount; i++) {
    if (idx) {
      a.fromBufferAttribute(pos, idx.getX(i * 3));
      b.fromBufferAttribute(pos, idx.getX(i * 3 + 1));
      c.fromBufferAttribute(pos, idx.getX(i * 3 + 2));
    } else {
      a.fromBufferAttribute(pos, i * 3);
      b.fromBufferAttribute(pos, i * 3 + 1);
      c.fromBufferAttribute(pos, i * 3 + 2);
    }
    volume += a.dot(b.cross(c)) / 6;
  }

  volume = Math.abs(volume);
  if (volume < 1) return `${volume.toFixed(3)} mm³`;
  if (volume < 1000) return `${volume.toFixed(1)} mm³`;
  return `${(volume / 1000).toFixed(1)} cm³`;
}

export default StlViewer;
