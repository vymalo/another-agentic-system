import Link from "next/link";

export default function NotFound() {
  return (
    <main className="standalone">
      <h1>Not found</h1>
      <p>
        There is nothing here. <Link href="/">Start a new thread</Link>.
      </p>
    </main>
  );
}
