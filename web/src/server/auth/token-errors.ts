/**
 * Thrown when a token could not be checked at all right now (the project's signing keys could
 * not be fetched, or a key the token names may have been published since the last fetch). It
 * says nothing about the token itself, so callers must not treat it as "signed out": the Mac
 * app's API answers 503 and the app keeps its session and retries later.
 */
export class TokenCheckUnavailable extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "TokenCheckUnavailable";
  }
}
