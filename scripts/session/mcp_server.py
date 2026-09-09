"""MCP is a transport for the same project-bound session reader."""
from __future__ import annotations
import json
from typing import Literal, Optional


def make_server(service):
    from mcp.server import MCPServer
    from mcp.types import ToolAnnotations
    from mcp.server.mcpserver.exceptions import ToolError
    from .store import BOOTSTRAP

    server = MCPServer("kpopper", instructions=BOOTSTRAP, log_level="WARNING")

    def run(function, *args):
        try:
            return function(*args)
        except (ValueError, KeyError, OSError) as error:
            raise ToolError(str(error)) from error

    @server.tool(structured_output=False, annotations=ToolAnnotations(readOnlyHint=True))
    def kpopper_open(tokens: int = 700) -> str:
        """Open the bound project's checked map within a reference-token budget."""
        return run(service.opening, tokens)

    @server.tool(structured_output=False, annotations=ToolAnnotations(readOnlyHint=True))
    def kpopper_read(ref: str, revision: str, tokens: int = 1600, offset: Optional[int] = None) -> str:
        """Read a returned reference at its revision; exact text supports labeled chunks."""
        return run(service.reading, ref, revision, tokens, offset)

    @server.tool(structured_output=False, annotations=ToolAnnotations(readOnlyHint=True))
    def kpopper_search(query: str, revision: str, ids: Optional[list[str]] = None,
                       limit: int = 8, tokens: int = 1000, branch: Optional[str] = None,
                       mode: Literal["lexical","semantic","hybrid"] = "hybrid") -> str:
        """Find candidate refs; known IDs win and branches never exclude stronger matches.

        Read returned refs for evidence. Local E5 is optional; any lexical fallback is explicit.
        """
        return run(service.searching,query,revision,tokens,ids,limit,branch,mode)

    @server.tool(structured_output=False, annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=False, idempotentHint=True))
    def kpopper_propose(revision: str, kind: Literal["observed", "inferred", "assumed", "question"],
                        text: str, basis: list[str], revisit: str = "") -> str:
        """Save a pending proposal. This does not change the canonical project record."""
        return run(service.proposing, revision, kind, text, basis, revisit)

    @server.tool(structured_output=False, annotations=ToolAnnotations(readOnlyHint=True))
    def kpopper_verify_claims(judgment: str, revision: str, assertions: list[dict]) -> str:
        """Check listed structured assertions only, not accompanying prose or world truth.

        Each assertion has kind, id and expected. Kinds: current, at_review,
        falsifier_holds, judgment_confidence, claim_text. at_review also needs dependency.
        """
        def verify():
            if not assertions:
                raise ValueError("no assertions supplied; nothing was checked")
            graph, _ = service.expect(revision)
            result = service.core.assess(graph.data, judgment, assertions)
            if result["assertions_accepted"] is not True:
                raise ValueError(json.dumps({"accepted": False, "checks": result["assertion_checks"]}))
            return json.dumps({"accepted": True, "checks": result["assertion_checks"],
                               "scope": "Only these assertions against this record, not prose or source-world truth."})
        return run(verify)

    return server
