// Adapted from knip's fixtures/class-members (knip 5.88.1, commit 8c25e4b7ff5c43d348082030600b1ba8677486af, ISC).
interface MyInterface {
  musthave: boolean;
  done(): void;
}

// to implement is to use
export class SomeClass implements MyInterface {
  musthave: boolean;
  constructor() {
    this.musthave = true;
  }
  done() {}
}
