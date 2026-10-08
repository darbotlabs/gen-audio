/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "1" brings the dev/test fixture deck and its assets back (same flag as PR #4). Unset in release builds. */
  readonly VITE_GEN_AUDIO_FIXTURES?: string;
}
