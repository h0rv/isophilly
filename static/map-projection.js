/** @typedef {{schema_version: 1, xScale: number, groundScale: number, heightScale: number}} MapProjection */

/** @type {Readonly<MapProjection>} */
export const IDENTITY_PROJECTION = Object.freeze({
  schema_version: 1,
  xScale: 1,
  groundScale: 1,
  heightScale: 1,
});

/** @param {MapProjection | undefined} projection */
export function isIdentityProjection(projection) {
  return (
    projection === undefined ||
    (projection.xScale === 1 && projection.groundScale === 1 && projection.heightScale === 1)
  );
}

/** Validate once at the input boundary, not once per mesh vertex.
 * @param {unknown} value
 * @returns {Readonly<MapProjection>}
 */
export function validateProjection(value) {
  if (value === undefined) return IDENTITY_PROJECTION;
  if (
    value === null ||
    typeof value !== "object" ||
    !("schema_version" in value) ||
    value.schema_version !== 1 ||
    !("xScale" in value) ||
    typeof value.xScale !== "number" ||
    !("groundScale" in value) ||
    typeof value.groundScale !== "number" ||
    !("heightScale" in value) ||
    typeof value.heightScale !== "number" ||
    ![value.xScale, value.groundScale, value.heightScale].every(
      (scale) => Number.isFinite(scale) && scale >= 0.5 && scale <= 2,
    )
  )
    throw new Error("Invalid camera projection: schema 1 requires finite scales from 0.5 to 2");
  return {
    schema_version: 1,
    xScale: value.xScale,
    groundScale: value.groundScale,
    heightScale: value.heightScale,
  };
}

/** Project real global vertices. Depth deliberately retains the original ordering.
 * @param {number} x
 * @param {number} groundY
 * @param {number} height
 * @param {MapProjection} [projection]
 * @returns {[number, number, number]}
 */
export function applyProjection(x, groundY, height, projection = IDENTITY_PROJECTION) {
  if (isIdentityProjection(projection)) return [x, groundY - height, groundY + height];
  return [
    x * projection.xScale,
    groundY * projection.groundScale - height * projection.heightScale,
    groundY + height,
  ];
}

/** Undo camera scales on the height-zero plane.
 * @param {number} x
 * @param {number} y
 * @param {MapProjection} [projection]
 * @returns {[number, number]}
 */
export function inverseProjection(x, y, projection = IDENTITY_PROJECTION) {
  if (isIdentityProjection(projection)) return [x, y];
  return [x / projection.xScale, y / projection.groundScale];
}
