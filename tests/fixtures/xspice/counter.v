// A 2-bit counter with a synchronous reset and a carry, for the Verilator tests of
// src/spice/xspice.
module counter(input clk, input rst, output reg [1:0] q, output carry);
  always @(posedge clk) begin
    if (rst) q <= 0;
    else q <= q + 1;
  end
  assign carry = &q;
endmodule
