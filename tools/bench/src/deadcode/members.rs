//! The member cases of the dead-code workspace: unused class, enum and namespace members, and live members that a
//! name-based analysis can get wrong. A member is dead when deleting it still type-checks and no executed code reads
//! it; every other member is live.

use std::fmt::Write as _;

use super::Package;

/// The classes of the member cases. Each name a dead member has is read on no object anywhere, except `Square.area`.
///
/// - `Ledger.archive`: read nowhere (dead).
/// - `Circle.area` and `Square.area`: one name on two unrelated classes; only `shape.area()` with `shape: Circle` reads
///   it, so `Square.area` is dead and a read by name keeps it (`member-same-name`).
/// - `Badge.label`: read only through `item.label()` with `item` typed by an interface `Badge` satisfies structurally
///   (no `implements` clause); deleting it breaks `show(new Badge())` (`member-interface`).
/// - `Parcel.weight`: read only through `item.weight` with `item: T` and `T extends HasWeight` (`member-generic`).
/// - `Frame.width`: read only by destructuring (`member-destructure`); `Frame.height`: read only as `frame[axis]` with
///   `axis: "height"`, a key of literal type that is no constant (`member-computed-literal`); `Frame.depth`: dead.
/// - `Animal.sleep`: read only through a `Dog` instance (`member-inherited`); `Animal.speak` and its override
///   `Dog.speak`: read only as `animal.speak()` with `animal: Animal`, which runs `Dog.speak` (`member-override`);
///   `Dog.fetch`: dead.
const MEMBER_CLASSES: &str = r#"export class Ledger {
  private readonly entries: number[] = [];

  record(value: number): number {
    this.entries.push(value);
    return this.entries.length;
  }

  archive(): number[] {
    return [...this.entries];
  }
}

export class Circle {
  radius = 1;

  area(): number {
    return 3 * this.radius * this.radius;
  }
}

export class Square {
  side = 2;

  area(): number {
    return this.side * this.side;
  }
}

export class Badge {
  label(): string {
    return "badge";
  }
}

export class Parcel {
  weight = 4;
}

export class Frame {
  width = 3;
  height = 4;
  depth = 5;
}

export class Animal {
  speak(): string {
    return "...";
  }

  sleep(): string {
    return "zzz";
  }
}

export class Dog extends Animal {
  override speak(): string {
    return "woof";
  }

  fetch(): string {
    return "ball";
  }
}
"#;

/// The enums of the member cases.
///
/// - `Mode.Legacy`: dead; `Mode.Fast` and `Mode.Slow` are read as `Mode.X`.
/// - `Tone.Cool`: read only through `palette.Cool` with `const palette: typeof Tone = Tone` (`enum-typed-alias`);
///   `Tone.Neon`: dead, though the enum object is assigned to a variable.
/// - `Size.Large`: read only as `Size[largest]` with `largest: keyof typeof Size` narrowed to `"Large"` by its
///   initializer (`enum-keyof-typeof`); `Size.Huge`: dead, though the enum is indexed by a key of type
///   `keyof typeof Size`.
const MEMBER_ENUMS: &str = r#"export enum Mode {
  Fast = "fast",
  Slow = "slow",
  Legacy = "legacy",
}

export enum Tone {
  Warm = "warm",
  Cool = "cool",
  Neon = "neon",
}

export enum Size {
  Small = "s",
  Large = "l",
  Huge = "xl",
}
"#;

/// The namespace of the member cases: `Geo.distance` is read only through a renamed import (`Maps.distance`),
/// `Geo.origin` only through an import alias (`import origin = Maps.origin`) (`namespace-aliased-import`), and
/// `Geo.pole` is dead.
const MEMBER_NAMESPACE: &str = r"export namespace Geo {
  export const origin = 0;

  export function distance(a: number, b: number): number {
    return Math.abs(a - b);
  }

  export const pole = 90;
}
";

/// Declarations the entry re-exports and only the next package reads (`member-cross-package`). An entry exposes
/// them, so no member of them is seeded dead: a tool may treat an entry's exports as public API.
const MEMBER_SHARED: &str = r#"export enum Level {
  Low = "low",
  High = "high",
}

export namespace Units {
  export const meter = 1;
  export const foot = 0.3048;
}

export class Gauge {
  reading(): number {
    return 1;
  }

  calibrate(): string {
    return "ok";
  }
}
"#;

impl Package {
    /// The member cases ([`MEMBER_CLASSES`], [`MEMBER_ENUMS`], [`MEMBER_NAMESPACE`], read by `src/members/use.ts`), and
    /// [`MEMBER_SHARED`] when the next package reads it. Returns the line the entry appends to re-export the shared
    /// declarations, empty when there are none.
    pub(super) fn members(&mut self, packages: usize) -> &'static str {
        let i = self.index;
        let shared = i + 1 < packages;
        self.file("src/members/classes.ts", MEMBER_CLASSES.to_owned());
        self.file("src/members/enums.ts", MEMBER_ENUMS.to_owned());
        self.file("src/members/ns.ts", MEMBER_NAMESPACE.to_owned());
        let classes = self.path("src/members/classes.ts");
        let enums = self.path("src/members/enums.ts");
        let namespace = self.path("src/members/ns.ts");
        for dead in ["Ledger.archive", "Square.area", "Frame.depth", "Dog.fetch"] {
            self.expect("classMembers", format!("{classes}|{dead}"));
        }
        for dead in ["Legacy", "Neon", "Huge"] {
            self.expect("enumMembers", format!("{enums}|{dead}"));
        }
        self.expect("namespaceMembers", format!("{namespace}|pole"));
        for (pattern, key) in [
            ("member-same-name", format!("{classes}|Circle.area")),
            ("member-interface", format!("{classes}|Badge.label")),
            ("member-generic", format!("{classes}|Parcel.weight")),
            ("member-destructure", format!("{classes}|Frame.width")),
            ("member-computed-literal", format!("{classes}|Frame.height")),
            ("member-inherited", format!("{classes}|Animal.sleep")),
            ("member-override", format!("{classes}|Animal.speak")),
            ("member-override", format!("{classes}|Dog.speak")),
            ("enum-typed-alias", format!("{enums}|Cool")),
            ("enum-keyof-typeof", format!("{enums}|Large")),
            ("namespace-aliased-import", format!("{namespace}|origin")),
            ("namespace-aliased-import", format!("{namespace}|distance")),
        ] {
            self.live(pattern, key);
        }
        if shared {
            self.file("src/shared.ts", MEMBER_SHARED.to_owned());
            let path = self.path("src/shared.ts");
            for name in ["Low", "High", "meter", "foot", "Gauge.reading", "Gauge.calibrate"] {
                self.live("member-cross-package", format!("{path}|{name}"));
            }
        }

        self.file("src/members/use.ts", use_module(i));
        if shared {
            "\nexport { Gauge, Level, Units } from \"./shared\";\n"
        } else {
            ""
        }
    }
}

/// `src/members/use.ts` of package `i`: reads every live member through its pattern, and the shared declarations of
/// the previous package when there is one.
fn use_module(i: usize) -> String {
    let mut text = String::from(concat!(
        "import { Animal, Badge, Circle, Dog, Frame, Ledger, Parcel, Square } from \"./classes\";\n",
        "import { Mode, Size, Tone } from \"./enums\";\n",
        "import { Geo as Maps } from \"./ns\";\n",
    ));
    if i > 0 {
        let _ = writeln!(text, "import {{ Gauge, Level, Units }} from \"@dc/p{}\";", i - 1);
    }
    text.push_str(concat!(
        "\n",
        "import origin = Maps.origin;\n",
        "\n",
        "interface Labelled {\n  label(): string;\n}\n",
        "\n",
        "interface HasWeight {\n  weight: number;\n}\n",
        "\n",
        "function measure(shape: Circle): number {\n  return shape.area();\n}\n",
        "\n",
        "function show(item: Labelled): string {\n  return item.label();\n}\n",
        "\n",
        "function heaviest<T extends HasWeight>(items: T[]): number {\n",
        "  return Math.max(...items.map((item) => item.weight));\n",
        "}\n",
        "\n",
        "function dimension(frame: Frame, axis: \"height\"): number {\n  return frame[axis];\n}\n",
        "\n",
        "function talk(animal: Animal): string {\n  return animal.speak();\n}\n",
        "\n",
        "export function exerciseMembers(): string {\n",
        "  const ledger = new Ledger();\n",
        "  const square = new Square();\n",
        "  const frame = new Frame();\n",
        "  const { width } = frame;\n",
        "  const dog = new Dog();\n",
        "  const palette: typeof Tone = Tone;\n",
        "  const largest: keyof typeof Size = \"Large\";\n",
        "  const parts: unknown[] = [\n",
        "    ledger.record(1),\n",
        "    measure(new Circle()),\n",
        "    square.side,\n",
        "    show(new Badge()),\n",
        "    heaviest([new Parcel()]),\n",
        "    width,\n",
        "    dimension(frame, \"height\"),\n",
        "    talk(dog),\n",
        "    dog.sleep(),\n",
        "    Mode.Fast,\n",
        "    Mode.Slow,\n",
        "    palette.Cool,\n",
        "    Tone.Warm,\n",
        "    Size[largest],\n",
        "    Size.Small,\n",
        "    Maps.distance(origin, 1),\n",
    ));
    if i > 0 {
        text.push_str(concat!(
            "    Level.Low,\n",
            "    Level.High,\n",
            "    Units.meter + Units.foot,\n",
            "    new Gauge().reading(),\n",
            "    new Gauge().calibrate(),\n",
        ));
    }
    text.push_str("  ];\n  return parts.join(\",\");\n}\n");
    text
}
