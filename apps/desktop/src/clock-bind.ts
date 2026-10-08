/** The cube clock follows the bound clip only. A null binding drives nothing. */
export function clipDrivesCube(activeClip: string | null, boundClip: string | null): boolean {
  return Boolean(activeClip) && boundClip !== null && activeClip === boundClip;
}
