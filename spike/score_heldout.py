"""Score held-out layouts: usage score_heldout.py <dir-with-md-outputs>. Expected text from heldout.py."""
import sys
d = sys.argv[1]
exp = {
 "narrow_unequal": [f"Main clause {i} sets out" for i in range(1,21)] + [f"Note {i} defines a term" for i in range(1,21)],
 "lower_half_columns": [f"Recital {i}:" for i in range(1,11)] + [f"Seller warranty {i} is given" for i in range(1,11)] + [f"Buyer covenant {i} survives" for i in range(1,11)],
 "block_quote": [f"Argument paragraph {i} explains" for i in range(1,8)] + [f"Quoted holding line {i} from" for i in range(1,6)] + [f"Argument paragraph {i} explains" for i in range(8,14)],
 "signature_block": [f"Body paragraph {i} states" for i in range(1,15)] + ["Dated: October 7, 2026","Respectfully submitted,","Jane Q. Counsel","Attorney for Plaintiff"],
 "four_up": [f"Question {k}.{ln} about the meeting" for k in range(1,5) for ln in range(1,26)],
}
for name, seq in exp.items():
    md = open(f"{d}/{name}.md").read()
    found = [p for p in (md.find(s) for s in seq) if p >= 0]
    inorder = sum(1 for a, b in zip(found, found[1:]) if b > a)
    print(f"{name:20s} found {len(found):3d}/{len(seq):3d}  in-order pairs {inorder}/{max(len(found)-1,0)}")
