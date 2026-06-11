# `@cccode/fxr`

Package reference snapshot for future effect-file research.

## What it is

`@cccode/fxr` is a JavaScript/TypeScript library for creating and editing FXR files: FromSoftware particle effects, lights, and related visual effect data. Its npm description lists support for Dark Souls 3, Sekiro, Elden Ring, Armored Core 6, and Elden Ring Nightreign.

This project currently applies named runtime SpEffect calls from Rust. FXR is adjacent reference material for file-level visual-effect exploration, not a direct replacement for runtime SpEffect invocation.

## Package snapshot

- npm package: <https://www.npmjs.com/package/@cccode/fxr>
- Registry package: <https://registry.npmjs.org/@cccode%2Ffxr>
- Latest version checked: `32.1.1`
- Published: 2026-05-25
- Install: `npm i @cccode/fxr`
- Runtime dependencies: none listed by npm
- Type declarations: built in (`./dist/fxr.d.ts`)
- License: Unlicense
- Repository: <https://github.com/EvenTorset/fxr>
- Documentation: <https://fxr-docs.pages.dev/>
- Playground: <https://fxr-playground.pages.dev/>

## README topics to inspect first

The package README is organized around these areas:

1. Try it out
2. Installation
3. Documentation
4. `fxrjson`
5. Editing FXR files
6. Creating new FXR files
7. Thanks

## Notes for this repo

- Useful when researching what existing Elden Ring FXR files contain or when prototyping edits in JavaScript before porting concepts elsewhere.
- The package exports schemas and data files, including `./schema`, `./schema/strict`, `./data/actions`, and `./data/enums`; these may be useful reference inputs if this Rust project later needs FXR parsing or validation concepts.
- Keep runtime SpEffect-call notes separate from FXR-file notes so the two effect layers do not get conflated.

## Last checked

- Date: 2026-06-10
- Source checked: npm package page and npm registry metadata
