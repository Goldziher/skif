// Adapted from knip's fixtures/class-members (knip 5.88.1, commit 8c25e4b7ff5c43d348082030600b1ba8677486af, ISC).
export abstract class AbstractClass {
  public abstract implemented(): AbstractClass;
  public abstract [Symbol.toStringTag](): AbstractClass;
}

export class ExtendedClass extends AbstractClass {
  public implemented(): AbstractClass {
    return this;
  }
}
