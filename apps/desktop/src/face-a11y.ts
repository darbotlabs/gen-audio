/** The face that is turned away is hidden from assistive tech, not only from sight. */
export function hiddenFace(flipped: boolean): { front: boolean; back: boolean } {
  return { front: flipped, back: !flipped };
}

export function backKindLabel(kind: string): string {
  return kind;
}
