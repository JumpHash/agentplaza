// A stylesheet imported as text: build.mjs's `ide-stylesheet-as-text` plugin turns styles/ide.css into this string, which
// the IDE view inserts when it first opens. Side-effect imports of the other stylesheets need no declaration.
declare module '*.css' {
  const text: string;
  export default text;
}
