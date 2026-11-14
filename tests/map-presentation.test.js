import assert from "node:assert/strict";
import test from "node:test";
import { createMapPresentation } from "../static/map-presentation.js";

class FakeContext {
  imageSmoothingEnabled = true;
  filter = "none";
  globalAlpha = 1;
  globalCompositeOperation = "source-over";
  fillStyle = "#000";
  transform = [1, 0, 0, 1, 0, 0];
  calls = [];
  stack = [];

  state() {
    return {
      imageSmoothingEnabled: this.imageSmoothingEnabled,
      filter: this.filter,
      globalAlpha: this.globalAlpha,
      globalCompositeOperation: this.globalCompositeOperation,
      fillStyle: this.fillStyle,
      transform: [...this.transform],
    };
  }

  save() {
    this.stack.push(this.state());
  }

  restore() {
    Object.assign(this, this.stack.pop());
  }

  setTransform(...values) {
    this.transform = values;
  }

  getImageData(_x, _y, width, height) {
    return { data: new Uint8ClampedArray(width * height * 4), width, height };
  }

  putImageData() {}

  fillRect(...args) {
    this.calls.push({ method: "fillRect", args, ...this.state() });
  }

  drawImage(...args) {
    this.calls.push({ method: "drawImage", args, ...this.state() });
  }
}

function withDocument(run) {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
  const canvases = [];
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      createElement(tag) {
        assert.equal(tag, "canvas");
        const context = new FakeContext();
        const canvas = {
          width: 300,
          height: 150,
          getContext(kind) {
            assert.equal(kind, "2d");
            return context;
          },
          context,
        };
        canvases.push(canvas);
        return canvas;
      },
    },
  });
  try {
    run(canvases);
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "document", descriptor);
    else delete globalThis.document;
  }
}

function layout(coverage = new Set(["8/100/100"])) {
  return { panX: -25, panY: 12, side: 2048, scale: 0.5, coverage, version: "v1" };
}

test("module imports without a DOM", () => {
  assert.equal(typeof createMapPresentation, "function");
});

test("resize uses the exact virtual CSS grid, independent of DPR", () => {
  withDocument((canvases) => {
    const presentation = createMapPresentation();
    presentation.resize(100, 80);
    assert.equal(presentation.canvas.width, 100);
    assert.equal(presentation.canvas.height, 80);
    assert.deepEqual(presentation.context.transform, [1, 0, 0, 1, 0, 0]);
    assert.equal(presentation.context.imageSmoothingEnabled, false);
    assert.equal(presentation.context.calls.at(-1).fillStyle, "#ded4c4");
    const calls = presentation.context.calls.length;
    presentation.resize(100, 80);
    assert.equal(presentation.context.calls.length, calls);
    assert.equal(canvases.length, 1);
    presentation.resize(0, Number.NaN);
    assert.equal(presentation.canvas.width, 1);
    assert.equal(presentation.canvas.height, 1);
  });
});

test("composite scopes color and nearest sampling without touching the retina transform", () => {
  withDocument(() => {
    const presentation = createMapPresentation();
    presentation.resize(150, 90);
    const target = new FakeContext();
    target.setTransform(2, 0, 0, 2, 0, 0);
    target.filter = "opacity(0.9)";
    target.globalAlpha = 0.7;
    target.globalCompositeOperation = "multiply";
    const before = target.state();
    presentation.drawTo(target, 150, 90, undefined);
    assert.deepEqual(target.state(), before);
    const composite = target.calls[0];
    assert.equal(composite.filter, "none");
    assert.equal(composite.imageSmoothingEnabled, false);
    assert.equal(composite.globalCompositeOperation, "source-over");
    assert.equal(composite.globalAlpha, 1);
    assert.deepEqual(composite.transform, [2, 0, 0, 2, 0, 0]);
    assert.deepEqual(composite.args, [presentation.canvas, 0, 0, 150, 90]);
    target.fillRect(0, 0, 10, 10); // A later overlay retains the original state.
    assert.equal(target.calls.at(-1).filter, before.filter);
  });
});

test("planned union fills enclosed holes and blurs only one padded alpha mask", () => {
  withDocument((canvases) => {
    const presentation = createMapPresentation();
    presentation.resize(150, 90);
    const coverage = new Set(["0/0/0", "8/256/1", "8/-1/1", "8/1/1/extra"]);
    for (let y = 100; y <= 102; y += 1) {
      for (let x = 100; x <= 102; x += 1) {
        if (x !== 101 || y !== 101) coverage.add(`8/${x}/${y}`);
      }
    }
    const before = presentation.context.state();
    presentation.drawTo(new FakeContext(), 150, 90, layout(coverage));
    assert.equal(canvases.length, 3);
    const [, padded, mask] = canvases;
    assert.equal(padded.width, 1040);
    assert.equal(mask.width, 1024);
    assert.equal(mask.height, 1024);
    assert.deepEqual(
      padded.context.calls.map((call) => call.args),
      [
        [408, 408, 12, 4],
        [408, 412, 12, 4],
        [408, 416, 12, 4],
      ],
    );
    assert.equal(mask.context.calls.length, 1);
    assert.equal(mask.context.calls[0].filter, "blur(2px)");
    assert.deepEqual(mask.context.calls[0].args, [padded, -8, -8]);
    const applied = presentation.context.calls.at(-1);
    assert.equal(applied.globalCompositeOperation, "destination-in");
    assert.equal(applied.imageSmoothingEnabled, true);
    assert.equal(applied.filter, "none");
    assert.deepEqual(applied.args, [mask, -25, 12, 1024, 1024]);
    assert.deepEqual(presentation.context.state(), before);
  });
});

test("mask cache depends on coverage identity and version, not camera movement", () => {
  withDocument((canvases) => {
    const presentation = createMapPresentation();
    const target = new FakeContext();
    const scene = layout();
    presentation.drawTo(target, 100, 100, scene);
    assert.equal(canvases.length, 3);
    presentation.drawTo(target, 100, 100, { ...scene, panX: 25, scale: 2 });
    assert.equal(canvases.length, 3);
    presentation.drawTo(target, 100, 100, { ...scene, version: "v2" });
    assert.equal(canvases.length, 5);
    presentation.drawTo(target, 100, 100, { ...scene, coverage: new Set(scene.coverage) });
    assert.equal(canvases.length, 7);
  });
});

test("missing metadata, absent z8 cells, and invalid layout keep imagery visible", () => {
  withDocument((canvases) => {
    const presentation = createMapPresentation();
    const target = new FakeContext();
    presentation.drawTo(target, 100, 100);
    presentation.drawTo(target, 100, 100, layout(new Set()));
    presentation.drawTo(target, 100, 100, layout(new Set(["0/0/0"])));
    presentation.drawTo(target, 100, 100, { ...layout(), scale: Number.NaN });
    assert.equal(canvases.length, 1);
    assert.equal(target.calls.length, 4);
    assert.equal(
      presentation.context.calls.some((call) => call.method === "drawImage"),
      false,
    );
    presentation.drawTo(target, 0, 100, layout());
    assert.equal(target.calls.length, 4);
  });
});
