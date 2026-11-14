// behind_text and darker, as the skeleton's engine theme computes them.
const rgb = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
const hex = c => "#" + c.map(v => v.toString(16).padStart(2, "0")).join("").toUpperCase();
const luminance = c => { const l = v => { v /= 255; return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }; return 0.2126 * l(c[0]) + 0.7152 * l(c[1]) + 0.0722 * l(c[2]); };
const contrast = (a, b) => { const x = luminance(a), y = luminance(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
const darker = c => c.map(v => Math.floor(v * 4 / 5));
function toHsl([r, g, b]) {
  [r, g, b] = [r, g, b].map(v => v / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2, ch = max - min;
  if (ch <= Number.EPSILON) return [0, 0, l];
  const s = ch / (1 - Math.abs(2 * l - 1));
  const six = max <= r ? (((g - b) / ch) % 6 + 6) % 6 : max <= g ? (b - r) / ch + 2 : (r - g) / ch + 4;
  return [six / 6, s, l];
}
function fromHsl(h, s, l) {
  const ch = (1 - Math.abs(2 * l - 1)) * s;
  const f = n => { const k = ((n + h * 12) % 12 + 12) % 12; const c = l - ch / 2 * Math.min(Math.max(Math.min(k - 3, 9 - k), -1), 1); return Math.round(Math.min(Math.max(c, 0), 1) * 255); };
  return [f(0), f(8), f(4)];
}
function behindText(c, dark) {
  const text = dark ? [255, 255, 255] : [0, 0, 0];
  let [h, s, l] = toHsl(c);
  l = dark ? Math.min(l, 0.5) : Math.max(l, 0.5);
  const step = dark ? -0.01 : 0.01;
  let out = fromHsl(h, s, l);
  for (let i = 0; i <= 100 && contrast(out, text) < 4.5; i++) {
    l = Math.min(Math.max(l + step, 0), 1);
    out = fromHsl(h, s, l);
  }
  return out;
}

function cell(h, index, dark) {
  const c = rgb(h), behind = hex(behindText(c, dark)), edge = hex(darker(c));
  return `<div class="cell" title="${index}: ${h} · border ${edge} · text background ${behind}"><span class="fill" style="background:${h};border-color:${edge}"></span><span class="txt" style="background:${behind};color:${dark ? "#FFFFFF" : "#000000"}">Aa</span><span class="i">${index}</span></div>`;
}
function look(range, dark) {
  let cells = (dark ? range.dark : range.light).map((h, i) => cell(h, i, dark)).join("");
  if (range.none) cells += cell(range.none[dark ? 0 : 1], "none", dark);
  return `<div><span class="label">${dark ? "Dark" : "Light"}</span><div class="ground ${dark ? "dark" : "light"}">${cells}</div></div>`;
}
const size = r => `${r.dark.length}${r.none ? ", and one for none" : ""}`;
document.getElementById("ranges-out").innerHTML = RANGES.map(r =>
  `<div class="range"><div class="range-head"><code>${r.name}</code><span>${size(r)}</span></div><div class="two">${look(r, true)}${look(r, false)}</div></div>`).join("");
