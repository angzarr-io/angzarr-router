/**
 * The domains a saga or process manager may issue commands to: the
 * component's declared output domains. Handlers address emitted commands to
 * these domains; the router stamps every emitted command deferred, so a
 * handler never sequences a command itself.
 */
export class Destinations {
  private readonly declared: readonly string[];

  /** Wraps the component's declared output domains (undefined becomes none). */
  constructor(domains?: Iterable<string>) {
    this.declared = Array.from(new Set(domains ?? []));
  }

  /** Whether `domain` is a declared output domain. */
  has(domain: string): boolean {
    return this.declared.includes(domain);
  }

  /** The declared output domains, in declaration order. */
  domains(): string[] {
    return [...this.declared];
  }
}
