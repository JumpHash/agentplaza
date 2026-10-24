export function greet(name: string): string {
  return `hello, ${name}`;
}

export function welcome(): string {
  return greet("plaza");
}
