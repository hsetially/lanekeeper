// The slice of the Trusted Types API the app uses. TypeScript's DOM library does not declare it. Types only.

interface TrustedHTML {
  toString(): string;
}
interface TrustedScriptURL {
  toString(): string;
}
interface TrustedTypePolicyOptions {
  createHTML?: (input: string, ...args: unknown[]) => string;
  createScriptURL?: (input: string, ...args: unknown[]) => string;
}
interface TrustedTypePolicy {
  readonly name: string;
  createHTML(input: string, ...args: unknown[]): TrustedHTML;
  createScriptURL(input: string, ...args: unknown[]): TrustedScriptURL;
}
interface TrustedTypePolicyFactory {
  createPolicy(name: string, options?: TrustedTypePolicyOptions): TrustedTypePolicy;
}
interface Window {
  trustedTypes?: TrustedTypePolicyFactory;
  /** Read by react-style-singleton (Radix scroll lock) when it makes a <style>. */
  __webpack_nonce__?: string;
}
