from hello import exports
from hello.types import Ok
from hello.imports.types import (
    IncomingRequest, ResponseOutparam,
    OutgoingResponse, Fields, OutgoingBody
)

class IncomingHandler(exports.IncomingHandler):
    def handle(self, _: IncomingRequest, response_out: ResponseOutparam):
        outgoingResponse = OutgoingResponse(Fields.from_list([]))
        outgoingResponse.set_status_code(200)
        outgoingBody = outgoingResponse.body()
        outgoingBody.write().blocking_write_and_flush(bytes("Hello from Python!\n", "utf-8"))
        OutgoingBody.finish(outgoingBody, None)
        ResponseOutparam.set(response_out, Ok(outgoingResponse))
