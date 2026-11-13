use proptest::prelude::*;

use super::{
    BinaryVia, FileTree, ScriptContext, ScriptInfo, is_ignored_binary, is_machine_binary, is_powershell_command,
    parse_script, without_commands,
};

type Scripts = &'static [(&'static str, &'static str)];
type Names = &'static [&'static str];

struct Case {
    script: &'static str,
    scripts: Scripts,
    bins: Names,
    files: Names,
    deps: Names,
    runs: Names,
}

const fn case(script: &'static str, bins: Names, files: Names, deps: Names) -> Case {
    Case {
        script,
        scripts: &[],
        bins,
        files,
        deps,
        runs: &[],
    }
}

const fn with_scripts(
    script: &'static str,
    scripts: Scripts,
    bins: Names,
    files: Names,
    deps: Names,
    runs: Names,
) -> Case {
    Case {
        script,
        scripts,
        bins,
        files,
        deps,
        runs,
    }
}

const fn bins(script: &'static str, bins: Names) -> Case {
    case(script, bins, &[], &[])
}

const fn files(script: &'static str, bins: Names, files: Names) -> Case {
    case(script, bins, files, &[])
}

const fn deps(script: &'static str, bins: Names, files: Names, deps: Names) -> Case {
    case(script, bins, files, deps)
}

fn context(scripts: Scripts) -> ScriptContext {
    scripts
        .iter()
        .fold(ScriptContext::new(), |ctx, (name, body)| ctx.with_script(*name, *body))
}

fn via_tag(via: BinaryVia) -> &'static str {
    match via {
        BinaryVia::Direct => "",
        BinaryVia::Npx => ":npx",
        BinaryVia::NpmExec => ":npm-exec",
        BinaryVia::PnpmExec => ":pnpm-exec",
        BinaryVia::Dlx => ":dlx",
        BinaryVia::Bunx => ":bunx",
        BinaryVia::YarnRun => ":yarn-run",
        BinaryVia::YarnExec => ":yarn-exec",
        BinaryVia::BunRun => ":bun-run",
    }
}

fn sorted(mut items: Vec<String>) -> Vec<String> {
    items.sort();
    items
}

fn expected(names: Names) -> Vec<String> {
    sorted(names.iter().map(|s| (*s).to_owned()).collect())
}

fn binaries_of(info: &ScriptInfo) -> Vec<String> {
    sorted(
        info.binaries
            .iter()
            .map(|b| format!("{}{}", b.name, via_tag(b.via)))
            .collect(),
    )
}

fn files_of(info: &ScriptInfo) -> Vec<String> {
    sorted(info.files.iter().map(|f| f.path.clone()).collect())
}

fn deps_of(info: &ScriptInfo) -> Vec<String> {
    sorted(
        info.dependencies
            .iter()
            .map(|d| {
                if d.ephemeral {
                    format!("{}:eph", d.name)
                } else {
                    d.name.clone()
                }
            })
            .collect(),
    )
}

fn parse(script: &str) -> ScriptInfo {
    parse_script(script, &ScriptContext::new())
}

#[test]
fn table() {
    let cases = vec![
        // Plain tools.
        bins("vitest run", &["vitest"]),
        bins("prettier --write .", &["prettier"]),
        bins("husky", &["husky"]),
        bins("husky install", &["husky"]),
        bins("lint-staged", &["lint-staged"]),
        bins("eslint . --fix", &["eslint"]),
        bins("rimraf dist && tsc", &["rimraf", "tsc"]),
        bins("echo hi && rm -rf dist && tsc -p tsconfig.json", &["tsc"]),
        bins("cd packages/a && eslint .", &["eslint"]),
        bins("tsc --noEmit || true", &["tsc"]),
        bins("", &[]),
        bins("   \t  ", &[]),
        bins("git commit -m \"feat: x\" && git push", &[]),
        bins("exit 1", &[]),
        bins("tsc && break || continue", &["tsc"]),
        bins("test -f a && vite", &["vite"]),
        // Environment assignments.
        bins("NODE_ENV=production webpack", &["webpack"]),
        bins("FOO=bar BAZ=\"a b\" vite build", &["vite"]),
        bins("FOO=1", &[]),
        files(
            "cross-env NODE_ENV=x tsx src/a.ts",
            &["cross-env", "tsx"],
            &["src/a.ts"],
        ),
        files(
            "cross-env NODE_OPTIONS=--require=./r.js jest",
            &["cross-env", "jest"],
            &["./r.js"],
        ),
        files("NODE_OPTIONS=\"--require ./setup.js\" jest", &["jest"], &["./setup.js"]),
        deps("NODE_OPTIONS='-r tsx/cjs' mocha", &["mocha"], &[], &["tsx"]),
        files(
            "cross-env-shell \"node a.js && vite\"",
            &["cross-env-shell", "vite"],
            &["a.js"],
        ),
        // Separators, subshells, substitutions, redirections, comments.
        bins("vite & tsc -w", &["vite", "tsc"]),
        bins("(cd a && vite) | tee log", &["vite"]),
        files("echo $(node scripts/v.js)", &[], &["scripts/v.js"]),
        bins("echo `tsc -v`", &["tsc"]),
        bins("$(npm bin)/vite build", &[]),
        bins("$CMD run", &[]),
        bins("${BIN} x", &[]),
        bins("echo ${HOME}/x && vite", &["vite"]),
        bins("vite build > out.log 2>&1", &["vite"]),
        bins("vite build &> /dev/null", &["vite"]),
        bins("tsc 2>/dev/null || echo fail", &["tsc"]),
        bins("vite # build with tsc", &["vite"]),
        bins("if [ -f a ]; then vite; fi", &["vite"]),
        bins("for f in a b; do tsc $f; done", &["tsc"]),
        bins("time vite build", &["vite"]),
        bins("env FOO=1 vite", &["vite"]),
        bins("exec vite", &["vite"]),
        bins("command -v foo", &[]),
        // Quoting and escapes.
        bins("echo \"a && b\" && vite", &["vite"]),
        bins("echo 'a ; b' ; vite", &["vite"]),
        bins("vite\\ build", &[]),
        bins("tsc \\\n  --noEmit\nvite build", &["tsc", "vite"]),
        bins("tsc && \\\nvite", &["tsc", "vite"]),
        // Unbalanced input stays best effort.
        bins("vite \"build", &["vite"]),
        bins("echo 'abc && vite", &[]),
        bins("echo $(vite", &["vite"]),
        bins("vite )) ((", &["vite"]),
        // Unicode.
        bins(
            "echo \"h\u{e9}llo \u{2713}\" && prettier --write \"src/**/\u{fc}.ts\"",
            &["prettier"],
        ),
        files("node ./scripts/\u{fc}n\u{ef}.js", &[], &["./scripts/\u{fc}n\u{ef}.js"]),
        // Paths.
        bins("./node_modules/.bin/eslint .", &["eslint"]),
        bins("node_modules/.bin/tsc", &["tsc"]),
        bins("../../node_modules/.bin/jest", &["jest"]),
        files("./scripts/build.sh", &[], &["./scripts/build.sh"]),
        bins("/usr/bin/foo", &[]),
        bins("sh -c \"vite build\"", &["vite"]),
        files("bash ./deploy.sh", &[], &["./deploy.sh"]),
        // node.
        files("node ./scripts/a.js --foo bar", &[], &["./scripts/a.js"]),
        deps("node --import tsx ./scripts/a.ts", &[], &["./scripts/a.ts"], &["tsx"]),
        deps(
            "node -r ts-node/register src/index.js",
            &[],
            &["src/index.js"],
            &["ts-node"],
        ),
        deps("node --require=dotenv/config app.js", &[], &["app.js"], &["dotenv"]),
        deps("node --loader ts-node/esm x.ts", &[], &["x.ts"], &["ts-node"]),
        deps(
            "node --import @swc-node/register/esm-register app.ts",
            &[],
            &["app.ts"],
            &["@swc-node/register"],
        ),
        files("node -r ./register.js app.js", &[], &["./register.js", "app.js"]),
        files("node --env-file=.env server.js", &[], &["server.js"]),
        files("node --experimental-strip-types src/a.ts", &[], &["src/a.ts"]),
        files("node -e \"console.log(1)\"", &[], &[]),
        files("node --import node:module ./a.mjs", &[], &["./a.mjs"]),
        // Runners.
        deps(
            "ts-node -r tsconfig-paths/register src/main.ts",
            &["ts-node"],
            &["src/main.ts"],
            &["tsconfig-paths"],
        ),
        files("tsx watch src/server.ts", &["tsx"], &["src/server.ts"]),
        files("tsx --tsconfig tsconfig.build.json src/a.ts", &["tsx"], &["src/a.ts"]),
        files("nodemon src/server.ts", &["nodemon"], &["src/server.ts"]),
        files(
            "nodemon --exec \"ts-node src/a.ts\" --watch src",
            &["nodemon", "ts-node"],
            &["src/a.ts"],
        ),
        files("ts-node-dev --respawn src/a.ts", &["ts-node-dev"], &["src/a.ts"]),
        files("vite-node scripts/seed.ts", &["vite-node"], &["scripts/seed.ts"]),
        files("zx scripts/a.mjs", &["zx"], &["scripts/a.mjs"]),
        files(
            "glob -c \"tsx scripts/check.ts\" \"src/**/*.ts\"",
            &["glob", "tsx"],
            &["scripts/check.ts"],
        ),
        deps(
            "babel-node -r dotenv/config src/a.js",
            &["babel-node"],
            &["src/a.js"],
            &["dotenv"],
        ),
        files(
            "esbuild src/index.ts --bundle --outfile=dist/out.js",
            &["esbuild"],
            &["src/index.ts"],
        ),
        deps(
            "mocha --require ts-node/register 'test/**/*.spec.ts'",
            &["mocha"],
            &["test/**/*.spec.ts"],
            &["ts-node"],
        ),
        deps("mocha -r ts-node/register", &["mocha"], &[], &["ts-node"]),
        files(
            "jest --config jest.config.js src/a.test.ts",
            &["jest"],
            &["src/a.test.ts"],
        ),
        files("vitest run src/a.test.ts", &["vitest"], &["src/a.test.ts"]),
        files("playwright test e2e/a.spec.ts", &["playwright"], &["e2e/a.spec.ts"]),
        // Wrappers.
        files("dotenv -e .env -- tsx a.ts", &["dotenv", "tsx"], &["a.ts"]),
        files("dotenvx run -- node a.js", &["dotenvx"], &["a.js"]),
        bins("env-cmd -f .env.test jest", &["env-cmd", "jest"]),
        files("c8 node dist/x.js", &["c8"], &["dist/x.js"]),
        bins("c8 check-coverage --lines 90", &["c8"]),
        bins("nyc --reporter=lcov mocha", &["nyc", "mocha"]),
        // npx, dlx, bunx.
        bins("npx tsc --noEmit", &["tsc:npx"]),
        bins("npx --no-install tsc", &["tsc:npx"]),
        bins("npx -c \"vitest run\"", &["vitest"]),
        deps(
            "npx -y create-react-app@latest my-app",
            &[],
            &[],
            &["create-react-app:eph"],
        ),
        deps("npx -p typescript tsc --init", &["tsc:npx"], &[], &["typescript:eph"]),
        deps(
            "npx --package=@scope/pkg@1 bin-name",
            &["bin-name:npx"],
            &[],
            &["@scope/pkg:eph"],
        ),
        deps("npx @angular/cli new app", &[], &[], &["@angular/cli:eph"]),
        files("npx ./scripts/run.js", &[], &["./scripts/run.js"]),
        files("npx tsx watch src/a.ts", &["tsx:npx"], &["src/a.ts"]),
        bins("pnpm dlx create-vite my-app", &["create-vite:dlx"]),
        deps("pnpm dlx -p typescript tsc", &["tsc:dlx"], &[], &["typescript:eph"]),
        bins("pnpx cowsay", &["cowsay:dlx"]),
        bins("yarn dlx -q eslint .", &["eslint:dlx"]),
        bins("bunx prettier --check .", &["prettier:bunx"]),
        bins("bunx --bun vite build", &["vite:bunx"]),
        bins("bun x vitest", &["vitest:bunx"]),
        // npm.
        bins("npm install", &[]),
        bins("npm exec -- vitest run", &["vitest:npm-exec"]),
        deps(
            "npm exec --package=typescript -- tsc",
            &["tsc:npm-exec"],
            &[],
            &["typescript:eph"],
        ),
        bins("npm run lint --workspace=web", &[]),
        bins("npm -w web run build", &[]),
        // pnpm.
        bins("pnpm install", &[]),
        bins("pnpm exec eslint .", &["eslint:pnpm-exec"]),
        bins("pnpm exec \"vitest run\"", &["vitest:pnpm-exec"]),
        bins("pnpm vitest", &["vitest:pnpm-exec"]),
        bins("pnpm -r exec tsc", &["tsc:pnpm-exec"]),
        bins("pnpm --filter web exec vitest", &["vitest:pnpm-exec"]),
        bins("pnpm --filter=web exec vitest", &["vitest:pnpm-exec"]),
        bins("pnpm -F web build", &[]),
        bins("pnpm --filter sub exec rimraf dist", &["rimraf:pnpm-exec"]),
        bins("pnpm --filter sub biome check", &["biome:pnpm-exec"]),
        bins("npm exec -w web -- vitest", &["vitest:npm-exec"]),
        // Wrappers forward to the command they run.
        bins("git ls-files | xargs prettier --write", &["prettier"]),
        bins("ls | xargs -n 1 -I {} eslint {}", &["eslint"]),
        bins("watch -n1 stylelint src", &["stylelint"]),
        bins("watch -n 1 stylelint src", &["stylelint"]),
        bins("nice -n 10 tsc", &["tsc"]),
        bins("sudo -u root vite", &["vite"]),
        bins("timeout 30 playwright test", &["playwright"]),
        bins("timeout -s KILL 30 playwright test", &["playwright"]),
        // A recursive run names scripts of the members, never a binary of this package.
        bins("pnpm -r build", &[]),
        bins("pnpm --recursive build", &[]),
        bins("pnpm -r --filter web build", &[]),
        bins("pnpm -r run build", &[]),
        bins("yarn workspaces foreach run build", &[]),
        bins("yarn workspaces foreach --all run build", &[]),
        bins("yarn workspaces foreach -A run build", &[]),
        bins("pnpm --filter web run build", &[]),
        bins("pnpm test", &[]),
        // yarn.
        bins("yarn", &[]),
        bins("yarn install --frozen-lockfile", &[]),
        bins("yarn vitest run", &["vitest:yarn-run"]),
        bins("yarn exec tsc", &["tsc:yarn-exec"]),
        files("yarn node scripts/a.js", &[], &["scripts/a.js"]),
        bins("yarn workspace web run build", &[]),
        bins("yarn workspace web exec vitest", &["vitest:yarn-exec"]),
        bins("yarn workspace web run biome check", &["biome:yarn-run"]),
        bins("yarn workspace web biome check", &["biome:yarn-run"]),
        bins("yarn workspaces foreach -A run build", &[]),
        // bun and corepack.
        files("bun run src/a.ts", &[], &["src/a.ts"]),
        files("bun src/b.ts", &[], &["src/b.ts"]),
        bins("bun test", &[]),
        bins("bun check --no-script", &[]),
        bins("bun install", &[]),
        bins("bun run vite", &["vite:bun-run"]),
        // `--filter` (`-F`) and `--workspaces` run a script of other workspace packages, not a binary here. A name
        // `bun run --filter` runs is recorded for the packages it names.
        bins("bun run --workspaces build", &[]),
        bins("bun run --parallel --workspaces --if-present lint", &[]),
        bins("bun -F web build", &[]),
        bins("bun --filter '*' build", &[]),
        bins("bun run -F './packages/*' --elide-lines 0 dev", &[]),
        // `--cwd` runs in another package: its scripts and binaries are that package's, its files are below it.
        bins("bun run --cwd packages/core fix-node-pty", &[]),
        bins("bun --cwd=packages/core run fix-node-pty", &[]),
        files(
            "bun run --cwd packages/cli src/index.ts",
            &[],
            &["packages/cli/src/index.ts"],
        ),
        files("bun --cwd ../cli src/b.ts", &[], &["../cli/src/b.ts"]),
        // `--prefix`, `--dir` and `-C` take a directory and aim pnpm at another package.
        bins("pnpm --prefix client vite", &[]),
        bins("pnpm -C client vite", &[]),
        // Arithmetic is not a command.
        bins("echo $((n+1))", &[]),
        bins("sleep $((RANDOM % 5)) && tsc", &["tsc"]),
        bins("((n++))", &[]),
        bins("for ((i=0;i<3;i++)); do vite; done", &["vite"]),
        bins("echo $( (cd a && vite) )", &["vite"]),
        bins("corepack pnpm exec vitest", &["vitest:pnpm-exec"]),
        bins("corepack enable", &[]),
        // Script references.
        with_scripts(
            "npm run build",
            &[("build", "tsc && vite build")],
            &["tsc", "vite"],
            &[],
            &[],
            &["build"],
        ),
        with_scripts(
            "npm run build -- --watch",
            &[("build", "tsc")],
            &["tsc"],
            &[],
            &[],
            &["build"],
        ),
        with_scripts(
            "npm run-script build",
            &[("build", "tsc")],
            &["tsc"],
            &[],
            &[],
            &["build"],
        ),
        with_scripts("npm test", &[("test", "jest")], &["jest"], &[], &[], &["test"]),
        // `--cwd` runs a script of the package there, not this package's script of the same name.
        with_scripts(
            "bun run --cwd packages/core build",
            &[("build", "tsc")],
            &[],
            &[],
            &[],
            &[],
        ),
        with_scripts("bun --cwd=packages/core build", &[("build", "tsc")], &[], &[], &[], &[]),
        with_scripts("npm t", &[("test", "jest")], &["jest"], &[], &[], &["test"]),
        with_scripts("npm run missing", &[], &[], &[], &[], &["missing"]),
        with_scripts("npm start", &[], &[], &[], &[], &["start"]),
        with_scripts("pnpm build", &[("build", "tsc")], &["tsc"], &[], &[], &["build"]),
        with_scripts("pnpm run build", &[("build", "tsc")], &["tsc"], &[], &[], &["build"]),
        with_scripts("pnpm -r run build", &[("build", "tsc")], &["tsc"], &[], &[], &["build"]),
        with_scripts("pnpm test", &[("test", "vitest")], &["vitest"], &[], &[], &["test"]),
        with_scripts("yarn run build", &[("build", "tsc")], &["tsc"], &[], &[], &["build"]),
        with_scripts("yarn build", &[("build", "tsc")], &["tsc"], &[], &[], &["build"]),
        with_scripts("bun run build", &[("build", "tsc")], &["tsc"], &[], &[], &["build"]),
        with_scripts("node --run build", &[("build", "tsc")], &["tsc"], &[], &[], &["build"]),
        with_scripts(
            "npm run build && node dist/index.js",
            &[("build", "tsc")],
            &["tsc"],
            &["dist/index.js"],
            &[],
            &["build"],
        ),
        with_scripts(
            "npm run a",
            &[("a", "npm run b"), ("b", "npm run a && vite")],
            &["vite"],
            &[],
            &[],
            &["a", "b"],
        ),
        with_scripts("npm run loop", &[("loop", "npm run loop")], &[], &[], &[], &["loop"]),
        with_scripts(
            "npm run a && npm run b",
            &[("a", "npm run c"), ("b", "npm run c"), ("c", "eslint .")],
            &["eslint"],
            &[],
            &[],
            &["a", "b", "c"],
        ),
        // Task runners.
        with_scripts(
            "concurrently \"npm:dev:*\" \"vite\"",
            &[("dev:a", "tsc -w"), ("dev:b", "sass --watch"), ("other", "jest")],
            &["concurrently", "vite", "tsc", "sass"],
            &[],
            &[],
            &["dev:a", "dev:b"],
        ),
        with_scripts(
            "concurrently -n a,b -c red,blue \"npm run a\" \"npm run b\"",
            &[("a", "vite"), ("b", "tsc -w")],
            &["concurrently", "vite", "tsc"],
            &[],
            &[],
            &["a", "b"],
        ),
        files(
            "concurrently --kill-others \"node a.js\" \"node b.js\"",
            &["concurrently"],
            &["a.js", "b.js"],
        ),
        bins("concurrently -k \"vite\" \"tsc -w\"", &["concurrently", "vite", "tsc"]),
        with_scripts(
            "run-p lint:*",
            &[("lint:js", "eslint ."), ("lint:css", "stylelint x"), ("build", "tsc")],
            &["run-p", "eslint", "stylelint"],
            &[],
            &[],
            &["lint:js", "lint:css"],
        ),
        with_scripts(
            "npm-run-all --parallel build:* test",
            &[("build:a", "tsc"), ("test", "jest")],
            &["npm-run-all", "tsc", "jest"],
            &[],
            &[],
            &["build:a", "test"],
        ),
        with_scripts(
            "run-s clean build",
            &[("clean", "rimraf dist"), ("build", "tsc")],
            &["run-s", "rimraf", "tsc"],
            &[],
            &[],
            &["clean", "build"],
        ),
        with_scripts(
            "turbo run build --filter=web",
            &[("build", "turbo run build")],
            &["turbo"],
            &[],
            &[],
            &[],
        ),
        // `lerna exec` and `nx exec` run their command in each member; `lerna run` and `nx run` take task names.
        bins("lerna exec -- rimraf dist", &["lerna", "rimraf"]),
        bins(
            "lerna exec --scope @fx/b --parallel -- unlistedbin --x",
            &["lerna", "unlistedbin"],
        ),
        bins("lerna exec --concurrency 2 --stream rimraf dist", &["lerna", "rimraf"]),
        bins("lerna exec -- \"rimraf dist && tsc\"", &["lerna", "rimraf", "tsc"]),
        bins("npx lerna exec -- rimraf dist", &["lerna:npx", "rimraf"]),
        bins("nx exec -- bazbin build", &["nx", "bazbin"]),
        bins("nx exec --projects a,b --parallel 2 -- tsc -b", &["nx", "tsc"]),
        bins("pnpm nx exec -- tsc", &["nx:pnpm-exec", "tsc"]),
        bins("lerna exec", &["lerna"]),
        bins("lerna run build --scope=@fx/a --stream", &["lerna"]),
        bins("lerna run test --since main --concurrency 2", &["lerna"]),
        bins("nx run web:build", &["nx"]),
        bins("nx run-many -t build lint", &["nx"]),
    ];
    for case in cases {
        let info = parse_script(case.script, &context(case.scripts));
        assert_eq!(binaries_of(&info), expected(case.bins), "binaries of {:?}", case.script);
        assert_eq!(files_of(&info), expected(case.files), "files of {:?}", case.script);
        assert_eq!(deps_of(&info), expected(case.deps), "dependencies of {:?}", case.script);
        assert_eq!(
            sorted(info.run_scripts.clone()),
            expected(case.runs),
            "scripts run by {:?}",
            case.script
        );
    }
}

#[test]
fn table_is_broad() {
    // Guards against the table above shrinking below the agreed coverage.
    let source = include_str!("tests.rs");
    let rows = source
        .lines()
        .filter(|l| {
            l.trim_start().starts_with("bins(")
                || l.trim_start().starts_with("files(")
                || l.trim_start().starts_with("deps(")
                || l.trim_start().starts_with("with_scripts(")
        })
        .count();
    assert!(rows >= 60, "only {rows} table rows");
}

#[test]
fn turbo_task_names_are_not_commands() {
    let ctx = context(&[("build", "tsc"), ("dev", "vite")]);
    for script in [
        "turbo run build --filter=web",
        "turbo run build lint --filter web",
        "turbo build",
        "turbo run test -- --coverage",
        "turbo prune web",
        "turbo watch dev",
    ] {
        let info = parse_script(script, &ctx);
        assert_eq!(binaries_of(&info), vec!["turbo".to_owned()], "{script}");
        assert!(
            info.files.is_empty() && info.dependencies.is_empty(),
            "{script}: {info:?}"
        );
        assert!(info.run_scripts.is_empty(), "{script}");
    }
}

#[test]
fn filtered_binaries_are_flagged() {
    let info = parse("pnpm --filter web exec vitest && pnpm exec eslint .");
    let flags: Vec<(&str, bool)> = info.binaries.iter().map(|b| (b.name.as_str(), b.filtered)).collect();
    assert_eq!(flags, [("vitest", true), ("eslint", false)]);
}

#[test]
fn bun_filter_runs_scripts_only_and_records_no_binary() {
    // Bun 1.4 runs only package scripts under `--filter`: `bun run --filter web tsc` fails with
    // `Script "tsc" not found in package "web"` when `web` has no `tsc` script.
    let used = |script: &str| -> Vec<String> { parse(script).binaries.iter().map(|b| b.name.clone()).collect() };
    for script in [
        "bun run --filter web vitest",
        "bun -F web run vitest",
        "bun --filter=web run vitest",
        "bun --filter web run -F api vitest",
        "bun run --workspaces vitest",
    ] {
        assert_eq!(used(script), Vec::<String>::new(), "{script}");
    }
    assert_eq!(used("bun run vitest"), ["vitest"]);
}

#[test]
fn ephemeral_vias() {
    assert!(BinaryVia::Npx.is_ephemeral());
    assert!(BinaryVia::Dlx.is_ephemeral());
    assert!(BinaryVia::Bunx.is_ephemeral());
    assert!(!BinaryVia::Direct.is_ephemeral());
    assert!(!BinaryVia::PnpmExec.is_ephemeral());
}

#[test]
fn npx_dependencies_are_ephemeral() {
    let info = parse("npx -p typescript tsc");
    assert!(info.dependencies.iter().all(|d| d.ephemeral));
    let info = parse("node -r tsx/cjs a.js");
    assert!(info.dependencies.iter().all(|d| !d.ephemeral));
}

#[test]
fn current_script_breaks_cycles() {
    let ctx = ScriptContext::new()
        .with_script("a", "npm run b")
        .with_script("b", "npm run a && vite")
        .with_current_script("a");
    let info = parse_script("npm run b", &ctx);
    assert_eq!(sorted(info.run_scripts.clone()), expected(&["a", "b"]));
    assert_eq!(binaries_of(&info), vec!["vite".to_owned()]);
}

#[test]
fn ignored_binaries() {
    for name in [
        "cd",
        "echo",
        "rm",
        "cp",
        "mkdir",
        "exit",
        "break",
        "continue",
        "test",
        "[",
        "true",
        "false",
        "git",
        "jq",
        "gh",
        "kubectl",
        "deno",
        "taskkill",
        "taskkill.exe",
        "xcode-select",
        "sysctl",
        "service",
        "update-rc.d",
        "c++",
        "signtool.exe",
        "powershell.exe",
        "osascript",
        "hg",
        "mkfifo",
        "mknod",
        "mktemp",
        "truncate",
        "unlink",
        "stdbuf",
        "uptime",
    ] {
        assert!(is_ignored_binary(name), "{name}");
    }
    // Tools that a base system lacks, and words a workspace's own command may well be named, stay binaries.
    for name in ["sqlite3", "stripe", "sentry-cli", "openssl", "relay-compiler", "users"] {
        assert!(!is_ignored_binary(name), "{name}");
    }
    for name in ["node", "tsc", "vite", "rimraf"] {
        assert!(!is_ignored_binary(name), "{name}");
    }
}

/// The Bazel launchers are binaries of the script, which credit the package that provides them, and machine tools.
#[test]
fn bazel_launchers_are_machine_binaries_not_ignored_ones() {
    let info = parse("bazel build //... && ibazel test //... && bazelisk run //:lint");
    let binaries: Vec<&str> = info.binaries.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(binaries, ["bazel", "ibazel", "bazelisk"]);
    assert_eq!(info.global_binaries, []);
    for name in ["bazel", "bazelisk", "ibazel"] {
        assert!(is_machine_binary(name), "{name}");
        assert!(!is_ignored_binary(name), "{name}");
    }
    assert!(!is_machine_binary("docker"));
}

#[test]
fn spans_point_at_the_binary() {
    let script = "echo hi && vitest run";
    let info = parse(script);
    assert_eq!(&script[info.binaries[0].span.clone()], "vitest");
}

#[test]
fn spans_inside_quoted_scripts_are_exact() {
    let script = "concurrently \"vite\" 'tsx a.ts'";
    let info = parse(script);
    let texts: Vec<&str> = info.binaries.iter().map(|b| &script[b.span.clone()]).collect();
    assert_eq!(texts, ["concurrently", "vite", "tsx"]);
    assert_eq!(&script[info.files[0].span.clone()], "a.ts");
}

#[test]
fn spans_inside_substitutions_are_exact() {
    let script = "echo $(vite build) && echo `tsc`";
    let info = parse(script);
    let texts: Vec<&str> = info.binaries.iter().map(|b| &script[b.span.clone()]).collect();
    assert_eq!(texts, ["vite", "tsc"]);
}

#[test]
fn spans_after_multibyte_text_are_exact() {
    let script = "echo \u{e9}\u{2713} && vite";
    let info = parse(script);
    assert_eq!(&script[info.binaries[0].span.clone()], "vite");
}

#[test]
fn spans_of_expanded_scripts_point_at_the_call_site() {
    let script = "npm run build";
    let ctx = ScriptContext::new().with_script("build", "tsc");
    let info = parse_script(script, &ctx);
    assert_eq!(&script[info.binaries[0].span.clone()], "build");
}

fn assert_spans_valid(script: &str, info: &ScriptInfo) {
    let spans = info
        .binaries
        .iter()
        .map(|b| &b.span)
        .chain(info.files.iter().map(|f| &f.span))
        .chain(info.dependencies.iter().map(|d| &d.span));
    for span in spans {
        assert!(script.get(span.clone()).is_some(), "bad span {span:?} in {script:?}");
    }
}

fn check(script: &str) {
    let ctx = ScriptContext::new()
        .with_script("a", "npm run b && tsc")
        .with_script("b", "npm run a && vite \"x y\"")
        .with_script("lint:x", "eslint .");
    let info = parse_script(script, &ctx);
    assert_spans_valid(script, &info);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn never_panics_on_arbitrary_text(script in ".*") {
        check(&script);
    }

    #[test]
    fn never_panics_on_shell_like_text(script in r#"[a-z0-9 "'\\$()`;&|<>#*=:@/.\n\t-]{0,120}"#) {
        check(&script);
    }

    #[test]
    fn never_panics_on_command_soup(
        script in proptest::collection::vec(
            prop_oneof![
                Just("npm run a"), Just("pnpm exec"), Just("npx -p"), Just("concurrently"), Just("\"npm:lint:*\""),
                Just("node -r"), Just("cross-env A=1"), Just("--"), Just("&&"), Just("$("), Just(")"), Just("'"),
                Just("run-p lint:*"), Just("turbo run"), Just("yarn workspace w"), Just("bun run"), Just("tsx watch"),
            ],
            0..24,
        )
    ) {
        check(&script.join(" "));
    }
}

#[test]
fn recursive_filtered_commands_run_elsewhere() {
    for script in [
        "pnpm -r --filter web exec vite",
        "pnpm --recursive --filter=web exec vite",
        "pnpm -r --filter web run build",
        "pnpm run -r build",
        "pnpm run build -r",
        "pnpm run build --filter web",
    ] {
        let info = parse(script);
        assert!(
            info.binaries.iter().all(|b| b.filtered) && info.run_scripts.is_empty(),
            "{script}: {info:?}"
        );
    }
}

#[test]
fn recursive_without_filter_still_runs_here() {
    let info = parse("pnpm -r exec vite");
    assert_eq!(binaries_of(&info), vec!["vite:pnpm-exec".to_owned()]);
    assert!(!info.binaries[0].filtered);
}

#[test]
fn member_exec_commands_run_here() {
    for script in [
        "lerna exec -- rimraf dist",
        "lerna exec --scope @fx/b -- rimraf dist",
        "nx exec -- rimraf dist",
    ] {
        let info = parse(script);
        let rimraf = info.binaries.iter().find(|b| b.name == "rimraf");
        assert!(
            rimraf.is_some_and(|b| !b.filtered && b.via == BinaryVia::Direct && b.dir.is_none()),
            "{script}: {info:?}"
        );
    }
}

#[test]
fn recursive_run_of_a_local_script_is_followed() {
    let ctx = context(&[("build", "tsc")]);
    let info = parse_script("pnpm run -r build", &ctx);
    assert_eq!(binaries_of(&info), vec!["tsc".to_owned()]);
}

#[test]
fn flags_and_subcommands_load_packages() {
    let cases: [(&str, &[&str]); 7] = [
        ("vitest --ui", &["@vitest/ui"]),
        (
            "vitest run --coverage",
            &["@vitest/coverage-istanbul", "@vitest/coverage-v8"],
        ),
        (
            "vitest run --coverage.enabled",
            &["@vitest/coverage-istanbul", "@vitest/coverage-v8"],
        ),
        ("webpack serve --mode development", &["webpack-dev-server"]),
        ("next lint --dir src", &["eslint", "eslint-config-next"]),
        ("next build", &[]),
        ("vitest run", &[]),
    ];
    for (script, expected_deps) in cases {
        assert_eq!(deps_of(&parse(script)), expected(expected_deps), "{script}");
    }
}

#[test]
fn node_modules_paths_are_packages_not_files() {
    let info = parse("node node_modules/@scope/tool/cli.js deploy && node ./node_modules/other/index.js");
    assert_eq!(deps_of(&info), expected(&["@scope/tool", "other"]));
    assert!(info.files.is_empty(), "{:?}", info.files);
}

#[test]
fn package_managers_are_recorded() {
    let info = parse("pnpm --filter web build && yarn lint && npm test && pnpm -r build");
    assert_eq!(info.package_managers, ["pnpm", "yarn", "npm"]);
    assert_eq!(parse("vitest").package_managers, [] as [std::string::String; 0]);
}

#[test]
fn test_runner_arguments_are_file_trees() {
    let info = parse("mocha --file ./build/test/setup.js --recursive --timeout 9000 ./build/test && node ./run.js");
    let trees: Vec<(&str, Option<FileTree>)> = info.files.iter().map(|f| (f.path.as_str(), f.tree.clone())).collect();
    let js = |recursive| FileTree {
        recursive,
        extensions: vec!["js".to_owned(), "cjs".to_owned(), "mjs".to_owned()],
    };
    assert_eq!(
        trees,
        [
            ("./build/test/setup.js", None),
            ("./build/test", Some(js(true))),
            ("./run.js", None)
        ]
    );
    assert_eq!(parse("mocha test").files[0].tree, Some(js(false)));
    assert_eq!(
        parse("mocha --extension ts --extension .tsx,mts test").files[0].tree,
        Some(FileTree {
            recursive: false,
            extensions: vec!["ts".to_owned(), "tsx".to_owned(), "mts".to_owned()],
        })
    );
}

#[test]
fn the_node_test_runner_is_detected_and_custom_reporters_are_packages() {
    assert!(parse("node --test").node_test);
    assert!(parse("cross-env A=1 node --test --test-reporter=spec test/").node_test);
    assert!(!parse("node app.js").node_test);
    let package_names = |script: &str| -> Vec<String> {
        parse(script)
            .dependencies
            .into_iter()
            .map(|dependency| dependency.name)
            .collect()
    };
    assert_eq!(
        package_names("node --test --test-reporter=custom-reporter"),
        ["custom-reporter"]
    );
    assert_eq!(package_names("node --test --test-reporter spec"), Vec::<String>::new());
    assert_eq!(package_names("node --test-reporter=lcov --test"), Vec::<String>::new());
}

#[test]
fn eslint_formatters_follow_the_naming_convention() {
    let deps = |script: &str| deps_of(&parse(script));
    assert_eq!(deps("eslint --format=codeframe ."), ["eslint-formatter-codeframe"]);
    assert_eq!(deps("eslint -f pretty ."), ["eslint-formatter-pretty"]);
    assert_eq!(deps("eslint --format @scope/x ."), ["@scope/eslint-formatter-x"]);
    assert_eq!(
        deps("eslint --format eslint-formatter-rdjson ."),
        ["eslint-formatter-rdjson"]
    );
    assert_eq!(deps("eslint --format stylish ."), [] as [&str; 0]);
    assert_eq!(deps("eslint --format ./formatter.js ."), [] as [&str; 0]);
}

#[test]
fn shell_builtins_are_neither_binaries_nor_global_binaries() {
    let names = |uses: &[super::BinaryUse]| uses.iter().map(|b| b.name.clone()).collect::<Vec<_>>();
    let info = parse("cd x && mapfile -t a < f && readarray b < f && caller && echo hi && rm -rf out && git pull");
    assert_eq!(names(&info.binaries), Vec::<String>::new());
    assert_eq!(names(&info.global_binaries), ["rm", "git"]);
}

#[test]
fn here_document_bodies_are_input_not_commands() {
    let script = "node <<'NODE'\nconst fs = require('fs');\nconsole.error(fs.readFileSync('x'))\nNODE\ntsc -p .";
    assert_eq!(binaries_of(&parse(script)), ["tsc"]);
    let script = "cat > ~/.netrc <<-EOF\n\tmachine github.com\n\tlogin x\n\tEOF\nvitest run";
    assert_eq!(binaries_of(&parse(script)), ["vitest"]);
    // Two documents opened on one line are read one after the other.
    let script = "cat <<A; cat <<\"B\"\nnot-a-bin\nA\nalso-not\nB\neslint .";
    assert_eq!(binaries_of(&parse(script)), ["eslint"]);
    // A here-string is a word, not a document; a document without its delimiter runs to the end.
    assert_eq!(binaries_of(&parse("grep x <<< \"$y\"\nprettier .")), ["prettier"]);
    assert_eq!(binaries_of(&parse("tsup\ncat <<EOF\nnot-a-bin\n")), ["tsup"]);
}

#[test]
fn case_patterns_are_not_commands() {
    let script = concat!(
        "case \"$(uname -s)\" in\n",
        "  Linux) tsc ;;\n",
        "  Darwin|FreeBSD)\n",
        "    vitest run\n",
        "    ;;\n",
        "  # a comment\n",
        "  *) exit 1 ;;\n",
        "esac\n",
        "eslint .",
    );
    assert_eq!(binaries_of(&parse(script)), ["eslint", "tsc", "vitest"]);
    // On one line, nested, with `;&`, and a last item without `;;`.
    let script = "case $a in x) case $b in y) prettier ;; esac ;; z) rimraf dist ;& *) tsup\nesac";
    assert_eq!(binaries_of(&parse(script)), ["prettier", "rimraf", "tsup"]);
    assert_eq!(
        binaries_of(&parse("case $? in 124|137) echo timeout ;; esac")),
        [] as [&str; 0]
    );
}

#[test]
fn functions_the_script_defines_are_not_binaries() {
    let script = concat!(
        "build_args() {\n",
        "  echo --filter\n",
        "}\n",
        "function hashes {\n",
        "  turbo run build --dry=json\n",
        "}\n",
        "log () { echo \"$1\"; }\n",
        "hashes && build_args && log hi && tsc",
    );
    assert_eq!(binaries_of(&parse(script)), ["tsc", "turbo"]);
    // A call before the definition names the function too.
    assert_eq!(binaries_of(&parse("setup; setup() { vitest run; }")), ["vitest"]);
}

#[test]
fn powershell_commands_are_cmdlets_and_default_aliases() {
    for name in [
        "New-Item",
        "Remove-Item",
        "Get-ChildItem",
        "Invoke-WebRequest",
        "gci",
        "ni",
        "iwr",
        "GCI",
    ] {
        assert!(is_powershell_command(name), "{name}");
    }
    for name in ["ts-node", "npm-run-all", "new-item", "New-", "-Item", "tsc", "turbo"] {
        assert!(!is_powershell_command(name), "{name}");
    }
}

#[test]
fn without_commands_blanks_the_dropped_commands_only() {
    let script = "New-Item -Path 'a b' -Force && tsc --build; gci | Remove-Item; vitest run";
    let kept = without_commands(script, is_powershell_command);
    assert_eq!(kept.len(), script.len());
    assert_eq!(binaries_of(&parse(&kept)), ["tsc", "vitest"]);
    assert_eq!(
        binaries_of(&parse(script)),
        ["New-Item", "Remove-Item", "gci", "tsc", "vitest"]
    );
}

/// The binaries of `script` with the directory each runs in, sorted.
fn binary_dirs(script: &str) -> Vec<(String, Option<String>)> {
    let mut found: Vec<(String, Option<String>)> = parse(script)
        .binaries
        .into_iter()
        .map(|binary| (binary.name, binary.dir))
        .collect();
    found.sort();
    found
}

fn at(name: &str, dir: Option<&str>) -> (String, Option<String>) {
    (name.to_owned(), dir.map(str::to_owned))
}

#[test]
fn commands_after_cd_run_in_its_directory() {
    assert_eq!(
        binary_dirs("cd sub && pnpm exec rspack serve"),
        [at("rspack", Some("sub"))]
    );
    assert_eq!(
        binary_dirs("tsc && cd packages/a && cd ../b && eslint . ; cd .. && vitest"),
        [
            at("eslint", Some("packages/b")),
            at("tsc", None),
            at("vitest", Some("packages"))
        ]
    );
    assert_eq!(
        binary_dirs("pushd web && vite build && popd && tsup"),
        [at("tsup", None), at("vite", Some("web"))]
    );
    // A directory that is not a static relative path is not followed.
    assert_eq!(
        binary_dirs("cd $DIR && tsc; cd /tmp && vite; cd ~ && tsup; cd - && eslint"),
        [at("eslint", None), at("tsc", None), at("tsup", None), at("vite", None)]
    );
    // A script of its own (`sh -c`, a substitution) keeps its `cd` to itself.
    assert_eq!(
        binary_dirs("sh -c 'cd a && vite' && tsc"),
        [at("tsc", None), at("vite", Some("a"))]
    );
    assert_eq!(binary_dirs("cd .. && cd sub && tsc"), [at("tsc", Some("../sub"))]);
    assert_eq!(binary_dirs("cd sub/.. && tsc"), [at("tsc", None)]);
}

/// A substitution runs in the directory of the command that holds it: after the `cd` before that command, before any
/// `cd` after it, and inside a subshell in the subshell's directory.
#[test]
fn a_substitution_runs_in_the_directory_of_its_command() {
    assert_eq!(
        binary_dirs("cd sub\nSUITE=$(pnpm exec tsx resolve.ts)"),
        [at("tsx", Some("sub"))]
    );
    assert_eq!(
        binary_dirs("echo $(vite) && cd sub && echo $(tsc) && cd .. && echo `eslint`"),
        [at("eslint", None), at("tsc", Some("sub")), at("vite", None)]
    );
    assert_eq!(
        binary_dirs("cd sub && echo $(cd deep && rspack) && tsup"),
        [at("rspack", Some("sub/deep")), at("tsup", Some("sub"))]
    );
    assert_eq!(
        binary_dirs("(cd sub && echo $(vite)) && echo $(tsc)"),
        [at("tsc", None), at("vite", Some("sub"))]
    );
}

#[test]
fn files_and_packages_after_cd_are_in_its_directory() {
    let info = parse("cd tools && node -r @x/hook ./build.js");
    assert_eq!(files_of(&info), ["tools/build.js"]);
    let dirs: Vec<(&str, Option<&str>)> = info
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.as_str(), dependency.dir.as_deref()))
        .collect();
    assert_eq!(dirs, [("@x/hook", Some("tools"))]);
}

#[test]
fn directory_flags_of_package_managers_set_the_directory() {
    for script in [
        "pnpm -C sub exec rspack",
        "pnpm --dir sub exec rspack",
        "npm --prefix sub exec rspack",
    ] {
        let info = parse(script);
        let binary = info.binaries.iter().find(|binary| binary.name == "rspack");
        assert_eq!(
            binary.map(|binary| (binary.dir.as_deref(), binary.filtered)),
            Some((Some("sub"), true)),
            "{script}: {:?}",
            info.binaries
        );
    }
    assert_eq!(
        binary_dirs("pnpm -C sub exec rspack && tsc"),
        [at("rspack", Some("sub")), at("tsc", None)]
    );
}

#[test]
fn directory_flags_below_an_unknown_directory_are_unknown() {
    for script in [
        "cd /tmp/app && pnpm -C packages/web exec rspack",
        "cd $DIR && pnpm --dir packages/web exec rspack",
        "cd /tmp/app && npm --prefix packages/web exec rspack",
        "cd /tmp/app && yarn --cwd packages/web exec rspack",
    ] {
        let info = parse(script);
        let binary = info.binaries.iter().find(|binary| binary.name == "rspack");
        assert_eq!(
            binary.map(|binary| (binary.dir.as_deref(), binary.filtered)),
            Some((None, true)),
            "{script}: {:?}",
            info.binaries
        );
    }
    // The flag's directory lasts for its command only.
    assert_eq!(
        binary_dirs("pnpm -C /abs exec rspack && cd sub && vite"),
        [at("rspack", None), at("vite", Some("sub"))]
    );
    assert_eq!(
        binary_dirs("pushd $OUT && popd && pnpm -C sub exec rspack"),
        [at("rspack", Some("sub"))]
    );
}

#[test]
fn a_script_run_in_another_directory_is_not_the_script_of_the_package() {
    let ctx = ScriptContext::new().with_script("build", "tsc -b");
    for script in [
        "cd packages/web && npm run build",
        "cd packages/web && pnpm build",
        "cd packages/web && pnpm run build",
        "cd packages/web && yarn build",
        "cd packages/web && yarn run build",
        "cd packages/web && bun run build",
        "cd packages/web && node --run build",
        "cd packages/web && run-s build",
        "pushd src && npm run build",
        "cd test/../web && npm run build",
    ] {
        let info = parse_script(script, &ctx);
        assert!(
            info.binaries.iter().all(|binary| binary.name != "tsc"),
            "{script}: {:?}",
            info.binaries
        );
        assert!(info.run_scripts.is_empty(), "{script}: {:?}", info.run_scripts);
    }
    // A binary run there by name is still one of that directory.
    let info = parse_script("cd packages/web && pnpm exec rspack", &ctx);
    assert_eq!(
        info.binaries
            .iter()
            .map(|binary| (binary.name.as_str(), binary.dir.as_deref()))
            .collect::<Vec<_>>(),
        [("rspack", Some("packages/web"))]
    );
    // A directory that leads back to the start runs the package's own script.
    let info = parse_script("cd test/.. && npm run build", &ctx);
    assert_eq!(judged_binaries(&info), ["tsc"], "{:?}", info.binaries);
    assert_eq!(info.run_scripts, ["build"]);
}

/// The binaries of `info` that are judged at the script's package: those run neither filtered nor in another directory.
fn judged_binaries(info: &ScriptInfo) -> Vec<String> {
    sorted(
        info.binaries
            .iter()
            .filter(|binary| !binary.filtered && binary.dir.is_none())
            .map(|binary| binary.name.clone())
            .collect(),
    )
}

#[test]
fn package_managers_after_cd_to_an_unknown_directory_work_on_the_package_there() {
    let ctx = ScriptContext::new().with_script("build", "tsc -b");
    for script in [
        "cd /tmp/app && bun run build",
        "cd /tmp/app && bun run compile",
        "cd $DIR && npm run build",
        "cd - && pnpm build",
        "cd /tmp && pnpm compile",
        "cd && yarn run build",
        "cd /tmp && yarn compile",
        "cd ~ && cd app && bun run compile",
        "pushd $OUT && pnpm exec rspack",
    ] {
        let info = parse_script(script, &ctx);
        assert!(judged_binaries(&info).is_empty(), "{script}: {:?}", info.binaries);
        assert!(info.run_scripts.is_empty(), "{script}: {:?}", info.run_scripts);
    }
    // A binary run by name there is still judged where the script starts, as before.
    assert_eq!(
        judged_binaries(&parse_script("cd /tmp && vitest run", &ctx)),
        ["vitest"]
    );
    // Back where the script starts, or in a static directory, the package's scripts are followed.
    for script in [
        "pushd /tmp && popd && npm run build",
        "sh -c 'cd /tmp' && npm run build",
        "cd sub && cd .. && npm run build",
    ] {
        let info = parse_script(script, &ctx);
        assert_eq!(judged_binaries(&info), ["tsc"], "{script}: {:?}", info.binaries);
        assert_eq!(info.run_scripts, ["build"], "{script}");
    }
    // Below an unknown directory, a relative one is unknown too.
    assert_eq!(binary_dirs("cd /tmp && cd sub && rspack"), [at("rspack", None)]);
}

#[test]
fn a_cd_in_a_subshell_ends_with_it() {
    let ctx = ScriptContext::new().with_script("build", "tsc -b");
    let info = parse_script("(cd \"$TMP\" && tar xf a.tgz) && npm run build", &ctx);
    assert_eq!(judged_binaries(&info), ["tsc"], "{:?}", info.binaries);
    assert_eq!(info.run_scripts, ["build"]);
    assert_eq!(
        binary_dirs("(cd sub && (cd deep && rspack) && eslint) && vitest"),
        [
            at("eslint", Some("sub")),
            at("rspack", Some("sub/deep")),
            at("vitest", None)
        ]
    );
    // A `{ … }` group runs in the same shell, so its `cd` lasts.
    assert_eq!(
        binary_dirs("{ cd sub; rspack; } && eslint"),
        [at("eslint", Some("sub")), at("rspack", Some("sub"))]
    );
}
