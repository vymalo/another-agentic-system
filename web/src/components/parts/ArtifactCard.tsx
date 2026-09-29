import type { ArtifactPartData } from "@/chat/to-items";
import { ActorLabel } from "../ActorLabel";

/** An artifact: a pull request becomes a link card, other URIs a link, inline text a disclosure. */
export function ArtifactCard({ data }: { data: ArtifactPartData }) {
  return (
    <section className="card" aria-label={`Artifact: ${data.name}`}>
      <div className="card__head">
        <span className="card__kind">Artifact</span>
        <ActorLabel actor={data.actor} />
      </div>
      {data.pr ? (
        <a className="card__link" href={data.pr.href} target="_blank" rel="noopener noreferrer">
          Pull request {data.pr.label}
        </a>
      ) : data.uri ? (
        <a className="card__link" href={data.uri} target="_blank" rel="noopener noreferrer">
          Open {data.name}
        </a>
      ) : (
        <p className="card__name">{data.name}</p>
      )}
      {data.text ? (
        <details className="card__details">
          <summary>{data.name}</summary>
          <pre>{data.text}</pre>
        </details>
      ) : null}
      {data.mimeType ? <p className="card__meta">{data.mimeType}</p> : null}
    </section>
  );
}
