#!/usr/bin/env node
import tk from 'terminal-kit';
const term = tk.terminal;

const eventLog = [];
const LOG_MAX = 12;

let pct = 50;
let r = 122, g = 64, b = 191;

function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }

function setColorFromPct() {
  const h = pct / 100;
  if (h < 0.33)      { const s = h / 0.33; r = Math.round(255 * (1 - s)); g = 0;           b = Math.round(255 * s); }
  else if (h < 0.67) { const s = (h - 0.33) / 0.34; r = 0;           g = Math.round(255 * s); b = Math.round(255 * (1 - s)); }
  else               { const s = (h - 0.67) / 0.33; r = Math.round(255 * s); g = Math.round(255 * (1 - s)); b = 0; }
}

setColorFromPct();

const TOP = 1;
const BOT = () => term.height;
const W = () => term.width;

function hline(row, col, len, ch) {
  term.moveTo(col, row);
  for (let i = 0; i < len; i++) term(ch || '─');
}

function drawPanel(y, x, w, h, title) {
  const tl = '╭', tr = '╮', bl = '╰', br = '╯', hh = '─', vv = '│';
  term.moveTo(x, y)(tl);
  for (let i = 1; i < w - 1; i++) term(hh);
  term(tr);
  for (let row = 1; row < h - 1; row++) {
    term.moveTo(x, y + row)(vv);
    term.moveTo(x + w - 1, y + row)(vv);
  }
  term.moveTo(x, y + h - 1)(bl);
  for (let i = 1; i < w - 1; i++) term(hh);
  term(br);
  if (title) {
    term.moveTo(x + 3, y).eraseLineAfter();
    term.dim(` ${title} `);
  }
}

function draw() {
  const w = W(), h = BOT();
  term.moveTo(1, 1).eraseDisplay();

  const PW = Math.max(w - 2, 40);
  const PX = Math.floor((w - PW) / 2) + 1;

  // outer panel
  term.colorRgb(100, 100, 100);
  drawPanel(TOP, PX, PW, h - 2, 'l-lightning · touch validation spike (M3.0)');
  term.styleReset();

  // ── header subtitle ──
  term.moveTo(PX + 3, TOP + 1).colorRgb(140, 140, 140)('drag the slider · tap to jump · ◄► arrows · q quit');

  // ══════ SLIDER ══════
  const sRow = TOP + 4;
  const sCol = PX + 4;
  const sW = PW - 10;
  const fillW = Math.round(sW * pct / 100);

  // Track
  term.moveTo(sCol, sRow);
  const trkHi = '━', trkLo = '─';
  for (let i = 0; i < sW; i++) {
    if (i < fillW) {
      const t = i / sW;
      const ir = Math.round(r * (0.3 + 0.7 * t));
      const ig = Math.round(g * (0.3 + 0.7 * t));
      const ib = Math.round(b * (0.3 + 0.7 * t));
      term.colorRgb(ir, ig, ib)(trkHi);
    } else {
      term.colorRgb(55, 55, 55)(trkLo);
    }
  }

  // Handle
  const hx = sCol + fillW;
  term.moveTo(hx, sRow).colorRgb(r, g, b).bold('◆').styleReset();

  // Pct label
  const lbl = `${pct}%`;
  const lblX = clamp(hx - Math.floor(lbl.length / 2) - 1, sCol, sCol + sW - lbl.length);
  term.moveTo(lblX, sRow + 1);
  term.colorRgb(r, g, b).bold(lbl).styleReset();

  // ── COLOR SWATCH ──
  const swRow = sRow + 4;
  const swCol = PX + 4;
  const swW = 14, swH = 5;

  // Double border swatch box
  term.colorRgb(80, 80, 80);
  term.moveTo(swCol, swRow)('╔');
  for (let i = 1; i < swW - 1; i++) term('═');
  term('╗');
  for (let row = 1; row < swH - 1; row++) {
    term.moveTo(swCol, swRow + row)('║');
    for (let i = 1; i < swW - 1; i++) term.bgColorRgb(r, g, b)(' ');
    term.styleReset().colorRgb(80, 80, 80)('║');
  }
  term.moveTo(swCol, swRow + swH - 1)('╚');
  for (let i = 1; i < swW - 1; i++) term('═');
  term('╝');
  term.styleReset();

  // Color info text
  const hex = `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${b.toString(16).padStart(2, '0')}`.toUpperCase();
  const infoCol = swCol + swW + 3;
  term.moveTo(infoCol, swRow + 1);
  term.colorRgb(r, g, b).bold(hex).styleReset();
  term.moveTo(infoCol, swRow + 2);
  term(`R ${String(r).padStart(3)}  G ${String(g).padStart(3)}  B ${String(b).padStart(3)}`);
  term.moveTo(infoCol, swRow + 3);
  term.colorRgb(120, 120, 120)(`pct ${pct}`);

  // ── WATERFALL GRADIENT BAR ──
  const gradRow = swRow + swH + 2;
  const gradCol = PX + 4;
  const gradW = Math.min(PW - 10, 60);
  term.moveTo(gradCol, gradRow);
  for (let i = 0; i < gradW; i++) {
    const t = i / gradW;
    let cr, cg, cb;
    if (t < 0.33)      { const s = t / 0.33; cr = 255 * (1 - s); cg = 0;          cb = 255 * s; }
    else if (t < 0.67) { const s = (t - 0.33) / 0.34; cr = 0;          cg = 255 * s; cb = 255 * (1 - s); }
    else               { const s = (t - 0.67) / 0.33; cr = 255 * s; cg = 255 * (1 - s); cb = 0; }
    term.bgColorRgb(Math.round(cr), Math.round(cg), Math.round(cb))(' ');
  }
  term.styleReset();
  term.moveTo(gradCol, gradRow + 1).colorRgb(120, 120, 120)('0%');
  term.moveTo(gradCol + gradW - 3, gradRow + 1).colorRgb(120, 120, 120)('100%');

  // ── EVENT LOG ──
  const logRow = gradRow + 3;
  term.moveTo(PX + 3, logRow).colorRgb(120, 120, 120)(`── events (${eventLog.length} · newest first) `);
  for (let i = 0; i < eventLog.length; i++) {
    term.moveTo(PX + 3, logRow + 1 + i);
    term(eventLog[i]);
  }

  // ── VERDICT ──
  const verRow = logRow + eventLog.length + 3;
  term.moveTo(PX + 3, verRow + 1);
  term.colorRgb(200, 160, 40)(`◆ `).styleReset()('touch tap:  ');
  term.colorGrayscale(18)('MOUSE_LEFT_BUTTON_PRESSED fires — slider position changes');
  term.moveTo(PX + 3, verRow + 2);
  term.colorRgb(200, 60, 60)(`◆ `).styleReset()('touch drag: ');
  term.colorGrayscale(18)('MOUSE_DRAG does NOT fire from touch on this system');

  // ── FOOTER ──
  term.moveTo(1, h);
}

let hasDrag = false;
let hasTouchTap = false;

function log(label, data) {
  const ts = new Date().toISOString().split('T')[1].slice(0, 12);
  const entry = `${ts}  ${label.padEnd(26)}  ${data}`;
  eventLog.unshift(entry);
  if (eventLog.length > LOG_MAX) eventLog.pop();
}

term.grabInput({ mouse: 'button' });
term.clear();
draw();

term.on('key', (name) => {
  if (name === 'CTRL_C' || name === 'q' || name === 'ESCAPE') {
    term.hideCursor(false);
    term.processExit(0);
    return;
  }
  if (name === 'LEFT' || name === 'RIGHT') {
    pct = clamp(pct + (name === 'LEFT' ? -5 : 5), 0, 100);
    setColorFromPct();
    draw();
  }
});

term.on('mouse', (name, data) => {
  const { x, y } = data;
  log(name, `x:${String(x).padStart(3)} y:${String(y).padStart(3)}`);

  if (name === 'MOUSE_DRAG') hasDrag = true;
  if (name === 'MOUSE_LEFT_BUTTON_PRESSED') hasTouchTap = true;

  if (name === 'MOUSE_DRAG' || name === 'MOUSE_LEFT_BUTTON_PRESSED') {
    const w = W();
    const PW = Math.max(w - 2, 40);
    const PX = Math.floor((w - PW) / 2) + 1;
    const sCol = PX + 4;
    const sW = PW - 10;
    const sRow = TOP + 4;
    const hitPad = 2;

    if (y >= sRow - hitPad && y <= sRow + hitPad && x >= sCol && x <= sCol + sW) {
      pct = clamp(Math.round((x - sCol) / sW * 100), 0, 100);
      setColorFromPct();
      draw();
    }
  }
});

process.on('exit', () => {
  term.styleReset();
  term.clear();
  term.hideCursor(false);
});
