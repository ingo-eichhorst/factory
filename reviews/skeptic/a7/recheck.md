# A7 targeted recheck — nine document findings resolved

Run: `82f3a4f4-24f5-48ce-b480-146ae12719a6`  
Date: 2026-09-09  
Prior review: `a7/review.md` and `a7/findings.csv`, preserved unchanged.

## Verdict

**A7S01–A7S09: all `behoben` at the level of the specific document findings.** The per-finding reasons and remaining boundaries are in [recheck.csv](recheck.csv). None remains open as the original mapping or verification defect.

This is not a renewed semantic approval of every case, an implementation assessment, or an operational authorization. Unimplemented external contracts are not silently promoted to surviving capabilities. Their explicit prerequisites and negative alternatives are what make the corrected document claims acceptable.

## Concrete results

- **S01:** S156.B01 retains a real blocked request, not an as-yet nonexistent clean package. Only appended S156.B03 receives GVR036. Existing disclosure-loss S156.B02 remains unchanged. Package bytes, permitted scope and send approval are distinct.
- **S02:** S074.B02 has KOR001 without OPR002. B03 explicitly retains the historical consumed attempt and a separate additional authorization revision. A new budget does not resolve the earlier effect or workspace ownership.
- **S03:** S054.B01/B02 now use the actual subscriber contract OBR029. M01 owns the retained range, M05 bounds transport. No runtime observation is invented and OPR014 is not merged away.
- **S04:** The two reproduction-above-service branches are `eskalation`; the two overlapping-writer branches are `konflikt`. No unsupported recurrent attractor or growth mechanism is added.
- **S05:** BDR037 now separates comparison, legitimate external decision and effective OS/participant exclusion. M12 remains a read-only witness. The powerless-witness alternative S350.B05 receives only OBR038, not BDR037.
- **S06:** Autonomous controller bindings have `support_modules=none`. O01–O03 separate prior Factory commands, local safety and later evidence import. The normal adapter credential chain is not a prerequisite for the local safety reaction.
- **S07:** AuthoritySource has an independent administrative owner, durable state outside restore, trust anchor and narrowly authorized query path. M02 checks fresh M16 evidence rather than treating M01 history, receipts or writer epochs as current revocation. S127.B03 explicitly lacks GVR011 when the source is missing. S191.B01 still requires both distinct authority and receipt contracts.
- **S08:** The compiler enforces the original freeze before parsing. Corrections bind its hash and expected original fields; appends preserve existing IDs. Both changed frozen bytes and in-memory S156 reversal are rejected.
- **S09:** Dependency validation is now in the compiler. The synthetic M07→M01 bootstrap back edge is rejected there, not merely by an unrelated test. The combined verifier reports its document-only scope honestly.

For S05–S07, `zielarchitektur.md` §7 explicitly makes the operation/facet tables limiting contracts for the general module-support lists. This matters: M13/M14 external recovery roles are not another running Factory kernel, and the independent authority query must not depend on the disputed old grant. Actual administrators, controllers and authority services still require separate ratification and evidence.

## Checks and scope

Read the prior findings/review and every correction file named in the assignment. Also inspected `bind_modules.py` before running its check, the affected generated branches, and the corresponding original candidate contracts. No full rereading of all 350 case cards was performed.

Executed from this scope:

```sh
python3 -B ../../docs/residuality/a7/verify.py
```

**Result:** 29 tests passed; 350 cases, 855 branches, 153 candidates and 16 modules. Of the candidates, 152 are independent originals and one is coordinator addition KOR001. The twelve frozen A7 originals and the earlier freeze checks passed. The changed-file negative test uses disposable copies, not altered originals.

Additional read-only, in-memory assertions checked counts, the negative S127/S350 mappings, unchanged S156 disclosure loss, and every original field of the targeted branches against the generated output except explicitly declared patches. They passed. These checks supplement manual reading; they do not simulate unavailable authority, physical controllers or a compromised runtime.

Reviewed revision anchors (SHA-256):

- `discovery-freeze.json`: `b70f2f068b525e15a323d5fc46accfe1fad5967a00090cc76c0c7e89a68a8ff9`
- `review-adjustments.json`: `ffb07ac3d367bfc7d67e9e7f1c3a854d7dcb27ffbb2ce14b0ef215ece0307751`

Only `a7/recheck.md` and `a7/recheck.csv` were authored. No original review, generated analysis, product code or configuration was edited. No external effects or live experiments were performed. No empirical attractor count or minimal global residue count follows from this recheck.
