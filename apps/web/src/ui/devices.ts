// Small SVG illustrations of the plaza's machines: what each one is.

export type DeviceKind = 'server' | 'workstation' | 'edge' | 'apple';

export interface DeviceOpts {
  /** Draw a small GPU card on the device. */
  gpu: boolean;
  /** VMs are running: LEDs lit/blinking. */
  active: boolean;
}

/** An SVG drawing (viewBox 0 0 120 90) of a machine of `kind`. */
export function deviceSvg(kind: DeviceKind, opts: DeviceOpts): SVGSVGElement {
  void kind; void opts;
  return document.createElementNS('http://www.w3.org/2000/svg', 'svg');
}
