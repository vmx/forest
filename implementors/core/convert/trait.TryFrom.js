(function() {var implementors = {};
implementors["fil_types"] = [{"text":"impl&lt;'_&gt; TryFrom&lt;&amp;'_ NodeType&gt; for Version","synthetic":false,"types":[]},{"text":"impl&lt;'_&gt; TryFrom&lt;&amp;'_ PieceInfo&gt; for PieceInfo","synthetic":false,"types":[]},{"text":"impl TryFrom&lt;RegisteredSealProof&gt; for RegisteredSealProof","synthetic":false,"types":[]},{"text":"impl TryFrom&lt;RegisteredPoStProof&gt; for RegisteredPoStProof","synthetic":false,"types":[]}];
implementors["forest_cid"] = [{"text":"impl TryFrom&lt;u64&gt; for Code","synthetic":false,"types":[]},{"text":"impl TryFrom&lt;String&gt; for Cid","synthetic":false,"types":[]},{"text":"impl&lt;'_&gt; TryFrom&lt;&amp;'_ str&gt; for Cid","synthetic":false,"types":[]},{"text":"impl TryFrom&lt;Vec&lt;u8&gt;&gt; for Cid","synthetic":false,"types":[]},{"text":"impl&lt;'_&gt; TryFrom&lt;&amp;'_ [u8]&gt; for Cid","synthetic":false,"types":[]}];
implementors["forest_libp2p"] = [{"text":"impl TryFrom&lt;TipsetBundle&gt; for Tipset","synthetic":false,"types":[]},{"text":"impl TryFrom&lt;TipsetBundle&gt; for CompactedMessages","synthetic":false,"types":[]},{"text":"impl TryFrom&lt;TipsetBundle&gt; for FullTipset","synthetic":false,"types":[]},{"text":"impl&lt;'_&gt; TryFrom&lt;&amp;'_ TipsetBundle&gt; for FullTipset","synthetic":false,"types":[]}];
implementors["key_management"] = [{"text":"impl TryFrom&lt;KeyInfo&gt; for Key","synthetic":false,"types":[]}];
if (window.register_implementors) {window.register_implementors(implementors);} else {window.pending_implementors = implementors;}})()