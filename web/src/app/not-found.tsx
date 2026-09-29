import Link from "next/link";

export default function NotFound() {
  return (
    <main className="mx-auto my-12 max-w-3xl px-4">
      <h1 className="text-xl font-semibold">Not found</h1>
      <p className="mt-2 text-muted-foreground">
        There is nothing here. <Link href="/">Start a new thread</Link>.
      </p>
    </main>
  );
}
