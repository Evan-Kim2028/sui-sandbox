"""
Shared A2A protocol middleware components.

This module contains middleware classes used by both the Green and Purple A2A agents
to implement protocol compliance.
"""

from __future__ import annotations

from typing import Any

from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response

from .constants import A2A_PROTOCOL_VERSION


class A2AVersionMiddleware(BaseHTTPMiddleware):
    """
    Middleware to add A2A-Version header to all responses.

    Implements A2A protocol version signaling per spec section 14.2.1.
    This ensures all HTTP responses include the protocol version header
    for client compatibility detection.
    """

    async def dispatch(self, request: Request, call_next: Any) -> Response:
        response = await call_next(request)
        response.headers["A2A-Version"] = A2A_PROTOCOL_VERSION
        return response
