/**
 * Slip: a ferry slip. A quay with a terminal and two berths, each with its
 * ramp. The live ferry has the bright stroke: its ramp is out, and cars leave
 * the terminal, cross the ramp and drive aboard, under the cabin. The handover
 * plays by itself: a new ferry backs in to the free berth; when it is healthy
 * its ramp runs out and the cars go to it in one step; the old ramp draws back
 * and the old ferry sails. Then the berths swap roles. Hovering slows the
 * clock, so the handover can be read. The slider is the hovered rate.
 *
 * The pattern: dilate time. Every pose is read off one clock, so the ferries,
 * the ramps and the cars slow together. A host can drive that clock with
 * `seek(ms)`: the Ferry landing page keeps it in step with its terminal, and
 * the moments below are those of its deploy/timeline.ts.
 */
const {
  Cam, EASE_LIFT, bezier, circ, clamp, facing, fillet, fit, poly, prism, proj, r2, rings,
  spring, stepS, reducedMotion, disposer, flatDot, mk, place, pointer, put, register, solid,
} = HL;

const FL = 58, DH = 7, CH = 9, LANE = [30, 74], G = 9, FAR = 34, RL = 14, BACK = 13, TK = 1.2;
// One handover, in ms: the new ferry comes in, it is healthy, the cars switch, the old ramp draws back, the old ferry sails.
const CYCLE = 13750, IN = [2600, 4950], OK = 7000, SWITCH = 7300, WITHDRAW = 8150, OUT = [8950, 11200];
const SPAWN = 200, TRIP = 1250;
/** How far in a ferry is, by its share of the time: fast from the offing, slow at the quay. A departure is the same run backwards. */
const DOCK = bezier(0.2, 0.6, 0.4, 1);

const smooth = (a) => { const t = clamp(a, 0, 1); return t * t * (3 - 2 * t); };
const ease = (a) => EASE_LIFT(clamp(a, 0, 1));
/** A closed convex outline round the origin, as a ring: each point with its outward normal. */
const ringOf = (pts) => pts.map((p, i) => {
  const a = pts[(i + pts.length - 1) % pts.length], b = pts[(i + 1) % pts.length];
  const nu = b[1] - a[1], nv = a[0] - b[0], k = (Math.sign(nu * p[0] + nv * p[1]) || 1) / (Math.hypot(nu, nv) || 1);
  return { u: p[0], v: p[1], nu: nu * k, nv: nv * k };
});
const inset = (ring, b) => ring.map((q) => ({ ...q, u: q.u - q.nu * b, v: q.v - q.nv * b }));
const circAt = (R, v) => circ(R, 14).map((q) => ({ ...q, v: q.v + v }));
/** A ring of the ferry's own plan, scaled by s and set down at (x, y). */
const at = (ring, x, y, s) => ring.map((q) => ({ ...q, u: x + q.u * s, v: y + q.v * s }));

// The ferry, in its own plan (u across, v along, the bow at +v): a hull that tapers to the bow, a cabin narrower than the hull, a wheelhouse and a funnel on it.
const HULL = ringOf(fillet([[-12, -29], [12, -29], [12, 5], [0, 29], [-12, 5]], [4, 4, 12, 3, 12], 6));
const PARTS = [
  [HULL, inset(HULL, 1.6), 0, DH],
  [...rings(-7.5, -9, 7.5, 15, 3.5, 1.2), DH, DH + CH],
  [...rings(-5.5, 7, 5.5, 14, 2.5, 0.9), DH + CH, DH + CH + 4.5],
  [circAt(2.6, -2), circAt(1.7, -2), DH + CH, DH + CH + 6],
];
const RAMP = fillet([[-4.5, 0], [4.5, 0], [4.5, RL], [-4.5, RL]], [0.8, 0.8, 2, 2]);

/** The road of a car bound for berth b: out of the terminal, along the quay, over the ramp and aboard. */
const road = (b) => [[52, -17], [52, -11], [LANE[b], -11], [LANE[b], 0], [LANE[b], RL], [LANE[b], G + 20]];
const ASHORE = 6 + 22 + 11, LEN = ASHORE + RL + (G + 20 - RL);
function along(pts, d) {
  for (let i = 1; ; i++) {
    const a = pts[i - 1], b = pts[i], l = Math.hypot(b[0] - a[0], b[1] - a[1]);
    if (d <= l || i === pts.length - 1) return [a[0] + ((b[0] - a[0]) * d) / l, a[1] + ((b[1] - a[1]) * d) / l];
    d -= l;
  }
}
/** The berth that takes the cars at time t: the old ferry's until the switch, then the new one's. */
function live(t) {
  const c = Math.floor(t / CYCLE), old = c % 2 ? 0 : 1;
  return t - c * CYCLE < SWITCH ? old : 1 - old;
}
const phase = (u) => (u < IN[0] ? "live" : u < IN[1] ? "docking" : u < OK ? "health" : u < WITHDRAW + 700 ? "switch" : u < OUT[1] ? "leaving" : "live");

function mount({ stage, svg, read }, value) {
  const bag = disposer();
  const C = Cam(45, 0.5, 2.4);
  // The quay, the two ferries at their longest reach, and the roof of the terminal.
  fit(C, [[8, -24, 0], [96, -24, 0], [96, 0, 0], [LANE[0] - 12, 75, 0], [LANE[1] + 12, 75, 0], [44, -23, DH + 8]], 200, 166);
  const P = proj(C), front = facing(C);
  let slow = value, over = false, clock = reducedMotion() ? 6200 : 2000, ext = null, said = "";
  const rate = spring(1, { eps: 0.002 });

  // Back to front: the quay, the cars on the quay, the terminal, then each berth.
  const g = mk("g", {}, svg);
  put(solid(g), prism(P, front, ...rings(8, -24, 96, 0, 5, 1.6), 0, DH));
  const ashore = mk("g", {}, g);
  put(solid(g), prism(P, front, ...rings(44, -23, 60, -15, 3, 1), DH, DH + 8));
  // A berth, back to front: the hull, the ramp (its edge, then its roadway), the cars on it and on board, the cabin and what stands on it.
  const berths = LANE.map(() => {
    const hull = solid(g), under = mk("path", { class: "lo" }, g), face = mk("path", { class: "sil" }, g), deck = mk("g", {}, g);
    const top = mk("g", {}, g), body = [hull, solid(top), solid(top), solid(top)];
    return { body, top, under, face, deck, key: null, out: NaN };
  });
  const cars = Array.from({ length: 8 }, () => ({ el: flatDot(ashore, C, 1.15, "dot m"), home: ashore }));
  const show = (el, on) => (on ? el.removeAttribute("visibility") : el.setAttribute("visibility", "hidden"));

  function drawBerth(b, c, u) {
    const B = berths[b], x = LANE[b], old = b === (c % 2 ? 0 : 1);
    // The ferry: an arrival grows in the offing and slows to the quay; a departure is the same run backwards.
    const k = old ? 1 - clamp((u - OUT[0]) / (OUT[1] - OUT[0]), 0, 1) : clamp((u - IN[0]) / (IN[1] - IN[0]), 0, 1);
    const s = smooth(k / 0.3), yc = G + FL / 2 + FAR * (1 - DOCK(k));
    const here = (old ? u < OUT[1] : u >= IN[0]) && s > 0.02, key = here ? r2(yc) + " " + r2(s) : "";
    if (key !== B.key) {
      B.key = key;
      show(B.body[0].g, here); show(B.top, here);
      if (here) PARTS.forEach(([ring, inner, z0, z1], i) => put(B.body[i], prism(P, front, at(ring, x, yc, s), at(inner, x, yc, s), z0 * s, z1 * s)));
    }
    // The ramp: out over the gap for the ferry that takes the cars, drawn back on the quay for the other berth.
    const out = r2(-BACK * (old ? ease((u - WITHDRAW) / 700) : 1 - ease((u - OK) / 700)));
    if (out !== B.out) {
      B.out = out;
      B.face.setAttribute("d", poly(RAMP.map((p) => P(x + p[0], out + p[1], DH + TK))));
      B.under.setAttribute("d", poly(RAMP.map((p) => P(x + p[0], out + p[1], DH))));
    }
    // The bright stroke is the live ferry's: the one the cars go to.
    B.body[0].sil.classList.toggle("hi", old ? u < SWITCH : u >= SWITCH);
  }

  function drawCars(t) {
    const n1 = Math.floor(t / SPAWN);
    for (let j = 0; j < cars.length; j++) {
      const n = n1 - j, car = cars[((n % 8) + 8) % 8], d = Math.min(((t - n * SPAWN) / TRIP) * LEN, LEN);
      // A car is gone at the end of its road. It takes the live berth's road when it reaches the fork, 6 units out of the terminal.
      show(car.el, d < LEN);
      const b = live(n * SPAWN + (6 / LEN) * TRIP), [x, y] = along(road(b), d), home = d <= ASHORE ? ashore : berths[b].deck;
      if (home !== car.home) { car.home = home; home.appendChild(car.el); }
      place(car.el, P(x, y, DH + (d > ASHORE && d < ASHORE + RL ? TK : 0)));
    }
  }

  const B = register(stage, (dt) => {
    const still = reducedMotion() && !over;
    rate.t = over ? slow : still ? 0 : 1;
    const moving = stepS(rate, dt);
    if (ext === null) clock += dt * 1000 * rate.x;
    const t = ext ?? clock, c = Math.floor(t / CYCLE), u = t - c * CYCLE;
    berths.forEach((_, b) => drawBerth(b, c, u));
    drawCars(t);
    const name = over ? phase(u) : "rest";
    if (name !== said) read.textContent = said = name;
    // Ambient: it asks for every frame, unless a host drives the clock or reduced motion holds it still.
    return ext === null && !(still && rate.x === 0) ? true : moving;
  });
  bag.add(B.unregister);
  bag.add(pointer(stage, { move: () => { over = true; B.wake(); }, leave: () => { over = false; B.wake(); } }));
  bag.add(() => svg.replaceChildren());

  return {
    set: (v) => { slow = v; B.wake(); },
    /** For a host that owns the clock: draws the handover at `ms`, and stops the figure's own clock. */
    seek: (ms) => { ext = ms; B.wake(); },
    /** How fast the clock runs now, from 1 down to the hovered rate: a host multiplies its own clock by it. */
    rate: () => rate.x,
    destroy: bag.dispose,
  };
}

hairline({
  name: "slip",
  means: "A new ferry docks beside the live one. When it is healthy, the cars go to it in one step and the old ferry sails.",
  rules: [4, 6, 7, 8],
  range: [0.6, 0.25, 0.08],
  mount,
});
