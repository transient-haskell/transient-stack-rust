{-# LANGUAGE OverloadedStrings #-}
-- runAt from a node to itself: the log and the closures it creates (to compare with transient-rs/transient-core/tests/run_at.rs)
import Transient.Internals
import Transient.Base
import Transient.Move.Internals
import Transient.Move.Logged
import Transient.Move.Utils
import Control.Monad.IO.Class
import Data.TCache
import Data.TCache.DefaultPersistence

main = keep $ initNode $ do
  node <- local getMyNode
  onAll $ do { l <- getLog; liftIO $ writeFile "hash0.txt" (show (hashClosure l)) }
  a <- local (return 1 :: TransIO Int)
  r <- runAt node (local (return 10 :: TransIO Int))
  onAll $ do
    l <- getLog
    liftIO $ print ("result", a + r)
    liftIO $ print ("hash", hashClosure l)
  localIO $ syncCache >> return ()
  onAll $ exit ()
