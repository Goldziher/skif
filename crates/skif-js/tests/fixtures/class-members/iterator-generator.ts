// Adapted from knip's fixtures/class-members (knip 5.88.1, commit 8c25e4b7ff5c43d348082030600b1ba8677486af, ISC).
export abstract class AbstractClassGen {
  protected constructor() {}

  public abstract implemented(): AbstractClassGen;

  public abstract unimplemented(): AbstractClassGen;

  public abstract [Symbol.iterator](): Iterator<AbstractClassGen>;
}

export class ExtendedClassGen extends AbstractClassGen {
  public constructor() {
    super();
    this;
  }

  public implemented(): AbstractClassGen {
    return this;
  }

  public *[Symbol.iterator](): Iterator<AbstractClassGen> {
    yield this;
  }
}
