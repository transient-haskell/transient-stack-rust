{-# LANGUAGE OverloadedStrings, ScopedTypeVariables #-}
-- minput with a POST body (POSTData), a list and a tuple: the descriptions of the endpoints and the responses, to compare
-- with transient-rs/transient-core/tests/web.rs
import Transient.Internals
import Transient.Base
import Transient.Move
import Transient.Move.Logged
import Transient.Move.Web
import Transient.Move.Utils
import Control.Monad.IO.Class
import Data.Default

main = keep $ initNode $ do
  minput "start" ("hello" :: String) :: Cloud ()
  POSTData (i :: Int) <- minput "endp" ("enter an int" :: String)
  localIO $ putStrLn $ "GOT " <> show i
  (a, b) <- minput "pair" ("pair" :: String) :: Cloud (Int, String)
  localIO $ putStrLn $ "PAIR " <> show (a, b)
  xs <- minput "list" ("a list" :: String) :: Cloud [Int]
  localIO $ putStrLn $ "LIST " <> show xs
  minput "end" ("done" :: String) :: Cloud ()
