import type { ValidateFunction } from "ajv";
import type { ViewportDocument } from "../validate";
import type { CatalogDocument } from "../library-assets";
export const viewportValidator: ValidateFunction<ViewportDocument>;
export const profileValidator: ValidateFunction;
export const catalogValidator: ValidateFunction<CatalogDocument>;
export const semanticCubeValidator: ValidateFunction<import("../semantic-cube").SemanticCube>;
