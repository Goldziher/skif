// Adapted from knip's fixtures/class-members (knip 5.88.1, commit 8c25e4b7ff5c43d348082030600b1ba8677486af, ISC).
import { SomeClass } from './implements';
import { MyClass, MyComponentClass } from './members';
import { AbstractClassGen, ExtendedClassGen } from './iterator-generator';
import { AbstractClass, ExtendedClass } from './iterator';

AbstractClass;

const instance = new MyComponentClass();
const some = new SomeClass();
const instance2 = new ExtendedClass();
const instance3: AbstractClassGen = new ExtendedClassGen();

export class Parent {
  instance: MyComponentClass;
  constructor() {
    this.instance = new MyComponentClass();

    // This member reference is only found because `this.instance` is a typed member of Parent
    this.instance.bUsedExternalFromTypedMemberInstance();
  }

  unusedMemberInEntry() {}
}

instance.bUsedExternal;
instance.cUsedExternal;
instance.dUsedExternal();
MyComponentClass.eUsedExternal;
instance2.implemented;
instance3.implemented;

MyClass.displayName;
