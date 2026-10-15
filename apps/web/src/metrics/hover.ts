// The shared crosshair: hovering one time chart moves a crosshair on every time chart of the view.

type Listener = (frac: number | null, source: unknown) => void;

export class HoverBus {
  private listeners = new Set<Listener>();

  /** Sets the hovered fraction of the x range (null: nothing hovered), naming the chart that set it. */
  set(frac: number | null, source: unknown): void {
    for (const fn of this.listeners) fn(frac, source);
  }

  on(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  clear(): void {
    this.listeners.clear();
  }
}
